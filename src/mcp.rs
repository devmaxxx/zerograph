//! `zerograph mcp`: the commands an agent shells out to, and the state of the index they read,
//! as tools of a stdio MCP server. The agent's problem today is not the answers but not knowing
//! whether they can be trusted: whether the index is behind the tree, whether a refresh is
//! running and how far it has got, which model wrote the vectors. `status` says all three.
//!
//! The wire is JSON-RPC 2.0, one message a line. stdout carries nothing else; every diagnostic a
//! library function prints goes to stderr, which the harness shows in its server log.

use crate::{ask, changes, config, impact, index, query, refresh, serve, status, store};
use anyhow::{bail, Context as _, Result};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

const PROTOCOL: &str = "2025-06-18";
/// Older revisions this server also speaks: the tools surface it uses is the same in each.
const ALSO_SPEAKS: [&str; 2] = ["2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

const INSTRUCTIONS: &str = "Ask the graph before grepping for a concept. `status` says whether the index is behind the tree and \
whether a background refresh is running; an answer given while files are behind says so itself. `reindex` starts a refresh \
and returns at once.";

struct Server {
    repo: PathBuf,
    no_dense: bool,
    /// The refresh this server started last, until its process exits. The child takes the writer
    /// lock only once it is running, so for that moment the lock says "none" of a refresh that
    /// is on its way.
    spawned: std::sync::Mutex<Option<refresh::Alive>>,
    /// Paid reranks this process has run, against the config's `mcp_reranks`.
    reranks: std::sync::atomic::AtomicU32,
}

pub fn run(repo: &Path, no_dense: bool) -> Result<()> {
    let server = Server::new(repo, no_dense);
    let mut out = std::io::stdout().lock();
    // Split on bytes rather than `lines()`: one message that is not UTF-8 would end the server
    // through `?`, and it is owed a parse error like any other malformed line.
    for raw in std::io::stdin().lock().split(b'\n') {
        let raw = raw?;
        let line = String::from_utf8_lossy(&raw);
        if line.trim().is_empty() { continue; }
        if let Some(reply) = server.handle_line(&line) {
            writeln!(out, "{reply}")?;
            out.flush()?;
        }
    }
    Ok(())
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// The agent may print what it is handed, so repository text reaches it as the terminal would.
fn text_result(text: String, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": crate::terminal_safe(&text) }], "isError": is_error })
}

impl Server {
    fn new(repo: &Path, no_dense: bool) -> Server {
        Server { repo: repo.to_path_buf(), no_dense, spawned: std::sync::Mutex::new(None), reranks: std::sync::atomic::AtomicU32::new(0) }
    }

    fn spawn_in_flight(&self) -> bool {
        let spawned = self.spawned.lock().unwrap_or_else(|e| e.into_inner());
        spawned.as_ref().is_some_and(|alive| alive.load(std::sync::atomic::Ordering::SeqCst))
    }

    fn remember(&self, alive: refresh::Alive) {
        *self.spawned.lock().unwrap_or_else(|e| e.into_inner()) = Some(alive);
    }

    /// The reply to one line, or `None` for a notification, which is owed none.
    fn handle_line(&self, line: &str) -> Option<String> {
        let reply = match serde_json::from_str::<Value>(line) {
            Ok(message) => self.handle(&message)?,
            Err(err) => error(Value::Null, PARSE_ERROR, &format!("parse error: {err}")),
        };
        Some(reply.to_string())
    }

    fn handle(&self, message: &Value) -> Option<Value> {
        // 2025-06-18 dropped JSON-RPC batching, so an array is not a request.
        let Some(object) = message.as_object() else {
            return Some(error(Value::Null, INVALID_REQUEST, "a request is a JSON object"));
        };
        let id = object.get("id").cloned();
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            // A response to a request this server never sent is not worth an answer either.
            if object.contains_key("result") || object.contains_key("error") { return None; }
            return id.map(|id| error(id, INVALID_REQUEST, "no method"));
        };
        let params = object.get("params").cloned().unwrap_or(Value::Null);
        let outcome = match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => self.call(&params),
            _ => Err((METHOD_NOT_FOUND, format!("no method {method:?}"))),
        };
        // No id is a notification; whatever it was, nobody is waiting for the outcome.
        let id = id?;
        Some(match outcome {
            Ok(value) => result(id, value),
            Err((code, message)) => error(id, code, &message),
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL);
        let version = if asked == PROTOCOL || ALSO_SPEAKS.contains(&asked) { asked } else { PROTOCOL };
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "zerograph", "version": env!("CARGO_PKG_VERSION") },
            "instructions": INSTRUCTIONS,
        })
    }

    /// A tool that fails is a result with `isError`, which the model reads and can act on; only a
    /// call that names no tool is a protocol error.
    fn call(&self, params: &Value) -> std::result::Result<Value, (i64, String)> {
        let name = params.get("name").and_then(Value::as_str).ok_or((INVALID_PARAMS, "tools/call needs a tool name".to_string()))?;
        if !tools().iter().any(|t| t["name"] == name) {
            return Err((INVALID_PARAMS, format!("no tool {name:?}")));
        }
        let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
        // A panic in an extractor or a renderer must cost one answer and not the session.
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.tool(name, &Args(&args))));
        Ok(match run {
            Ok(Ok(text)) => text_result(text, false),
            Ok(Err(err)) => text_result(format!("{err:#}"), true),
            Err(_) => text_result(format!("{name} failed unexpectedly; the server's stderr has the reason"), true),
        })
    }

    fn tool(&self, name: &str, args: &Args) -> Result<String> {
        match name {
            "ask" => self.ask(args),
            "explain" => self.explain(args),
            "impact" => self.impact(args),
            "trace" => self.trace(args),
            "changes" => self.changes(args),
            "status" => self.status(),
            "reindex" => self.reindex(),
            "cache_status" => crate::cache_command(crate::CacheAction::Status),
            "switch_model" => self.switch_model(args),
            _ => bail!("no tool {name:?}"),
        }
    }

    /// Read per call, so an edit to `zerograph.toml` is the next answer's and a broken one is an
    /// error the agent can pass on rather than a server that never started.
    fn config(&self) -> Result<config::Config> {
        let cfg = config::Config::load(&self.repo)?;
        crate::cap_pools(index::embed::threads(cfg.resources));
        Ok(cfg)
    }

    fn ask(&self, args: &Args) -> Result<String> {
        let question = args.string("question")?;
        if question.len() > serve::HELLO_MAX { bail!("the question is {} bytes; the most one can be is {}", question.len(), serve::HELLO_MAX); }
        let (rerank, rerank_local) = (args.flag("rerank"), args.flag("rerank_local"));
        if rerank && rerank_local { bail!("rerank and rerank_local are two ways to pick the seeds; name one"); }
        let mut req = ask::Request {
            words: question.split_whitespace().map(str::to_string).collect(),
            json: false,
            seeds: args.number("seeds", 5)?,
            bodies: args.flag("bodies"),
            rerank,
            rerank_local,
            depth: args.number("depth", crate::rerank::DEPTH)?,
            stale: args.flag("stale"),
            no_dense: self.no_dense,
        };
        if req.words.is_empty() { bail!("the question is empty"); }
        let cfg = self.config()?;
        let mut spent = Vec::new();
        req.rerank = req.rerank && self.may_rerank(cfg.mcp_reranks, &mut spent);
        let (text, notices) = self.answer(&req, &cfg)?;
        Ok(with_notices(text, spent.into_iter().chain(notices).collect()))
    }

    /// Charges the allowance by the commands each attempt started, whether or not they answered
    /// and whether or not the answer itself failed: a model that timed out or failed may still
    /// have billed, and an exact id or a refused command never reached one.
    fn answer(&self, req: &ask::Request, cfg: &config::Config) -> Result<(String, Vec<String>)> {
        let charge = |n: u32| self.reranks.fetch_add(n, std::sync::atomic::Ordering::SeqCst);
        if crate::env_os("NO_SERVE").is_none() {
            let (resident, failed) = serve::try_ask_spending(&self.repo, req);
            charge(failed);
            if let Some(reply) = resident {
                charge(reply.reranked);
                return Ok((reply.stdout, reply.stderr));
            }
        }
        let mut ctx = ask::Context::open(&self.repo, cfg, req.stale, self.no_dense)?;
        let answered = ctx.answer(req);
        charge(ctx.take_reranked());
        Ok((answered?, ctx.notices()))
    }

    /// Whether this process's allowance has a paid rerank left, or why there is none.
    fn may_rerank(&self, limit: u32, notices: &mut Vec<String>) -> bool {
        let left = self.reranks.load(std::sync::atomic::Ordering::SeqCst) < limit;
        if !left {
            notices.push(format!("rerank: this server has run its {limit} paid reranks (mcp_reranks in the machine config); answering from the fused order"));
        }
        left
    }

    fn explain(&self, args: &Args) -> Result<String> {
        let node = args.string("node")?;
        let (graph, _, notes) = crate::graph_with_notes(&self.repo, &self.config()?, args.flag("stale"), self.no_dense)?;
        Ok(with_notices(query::explain(&graph, &node)?, notes))
    }

    fn impact(&self, args: &Args) -> Result<String> {
        let symbol = args.string("symbol")?;
        let depth = args.number("depth", 3)?;
        let (graph, _, notes) = crate::graph_with_notes(&self.repo, &self.config()?, args.flag("stale"), self.no_dense)?;
        let root = crate::code_node(&graph, &symbol)?;
        let text = match args.flag("down") {
            true => impact::render(&graph, &impact::downstream(&graph, &root.id, depth), "downstream"),
            false => impact::render(&graph, &impact::upstream(&graph, &root.id, depth), "upstream"),
        };
        Ok(with_notices(text, notes))
    }

    fn trace(&self, args: &Args) -> Result<String> {
        let (from, to) = (args.string("from")?, args.string("to")?);
        let depth = args.number("depth", 6)?;
        let (graph, _, notes) = crate::graph_with_notes(&self.repo, &self.config()?, args.flag("stale"), self.no_dense)?;
        let (a, b) = (crate::code_node(&graph, &from)?, crate::code_node(&graph, &to)?);
        // The CLI exits 3 on no path; here it is the answer to the question, not a failed call.
        let text = match impact::trace(&graph, &a.id, &b.id, depth) {
            Some(path) => crate::trace_text(&graph, &path),
            None => format!("no call path from {} to {} within {depth} hops\n", a.id, b.id),
        };
        Ok(with_notices(text, notes))
    }

    fn changes(&self, args: &Args) -> Result<String> {
        let base = args.optional_string("base").unwrap_or_else(|| "HEAD".to_string());
        let depth = args.number("depth", 2)?;
        let (graph, _, notes) = crate::graph_with_notes(&self.repo, &self.config()?, args.flag("stale"), self.no_dense)?;
        let report = changes::report(&graph, &changes::hunks_from_git(&self.repo, &base)?, depth);
        Ok(with_notices(changes::render(&graph, &report), notes))
    }

    fn status(&self) -> Result<String> {
        let report = status::report(&self.repo, self.spawn_in_flight())?;
        Ok(status::render(&report, &self.repo, "`reindex`"))
    }

    fn reindex(&self) -> Result<String> {
        let store = store::Store::new(&self.repo);
        // Asked before the lock, which makes the directory it would be asking about.
        let run = refresh_command(&store);
        match (store.try_lock_writer()?, self.spawn_in_flight()) {
            (None, _) => Ok(format!("a refresh is already running ({}); `status` follows it", progress(&self.repo))),
            (Some(_), true) => Ok("a refresh is already running (starting); `status` follows it".to_string()),
            (Some(lock), false) => {
                // Released first: the child takes the lock itself, and waits on this process for it
                // if it is still held when the child asks.
                drop(lock);
                self.remember(refresh::spawn_detached(&self.repo, self.no_dense, &[run])?);
                Ok(format!("`{run}` started in the background; `status` shows how far it has got"))
            }
        }
    }

    fn switch_model(&self, args: &Args) -> Result<String> {
        let id = args.string("model")?;
        let id = id.trim();
        if id.is_empty() { bail!("name a hub id, e.g. {}", index::embed::RECOMMENDED); }
        // The caller is an agent, and the agent may have read the id out of a repository. A model
        // is code this machine downloads and hands to a parser, so only a person trusts a new one
        // (`zerograph model <id>`, which records it); the catalogue's are measured and need none.
        if !config::model_is_trusted(id) {
            bail!("{id} is not a catalogued model and this machine has not trusted it, so it is not downloaded on an agent's word. \
                   Ask the user to run `zerograph model {id}` once in a terminal; after that this tool accepts it. Catalogued: {}.",
                  index::embed::RECOMMENDED);
        }
        if self.no_dense { bail!("this server runs with --no-dense, and a model switch is the dense index"); }
        let store = store::Store::new(&self.repo);
        let recorded = index::dense::DenseIndex::recorded_model(&store)?;
        if recorded.as_deref().is_some_and(|m| m.eq_ignore_ascii_case(id)) && self.config()?.embed_model.eq_ignore_ascii_case(id) {
            return Ok(format!("the store is already on {id}; nothing to do"));
        }
        match (store.try_lock_writer()?, self.spawn_in_flight()) {
            (None, _) => bail!("a refresh is running ({}); switch once it is over, so the two do not write the same rows", progress(&self.repo)),
            (Some(_), true) => bail!("a refresh is starting; switch once it is over, so the two do not write the same rows"),
            (Some(lock), false) => drop(lock),
        }
        // The CLI's own switch, detached: it opens the model before it writes anything, so an id
        // the hub cannot serve leaves the project as it was and says why in the log.
        self.remember(refresh::spawn_detached(&self.repo, false, &["model", id])?);
        Ok(format!("switching to {id} in the background: the model is fetched and opened, then `{}` is written and the vectors rewritten. \
                    `status` follows it; `{}/background.log` has the reason if it fails.",
                   config::project_file_name(&self.repo), store::dir_name_of(&self.repo)))
    }
}

/// An answer followed by what was said about it on the way, which a terminal user reads on
/// stderr and an agent would otherwise never see.
fn with_notices(mut text: String, notices: Vec<String>) -> String {
    if notices.is_empty() { return text; }
    if !text.ends_with('\n') { text.push('\n'); }
    text.push('\n');
    for n in notices { text.push_str(&n); text.push('\n'); }
    text
}

/// What a `reindex` runs: the first `build` where no store stands, else an `update`, each taking
/// the writer lock in its own process as it does on the command line.
fn refresh_command(store: &store::Store) -> &'static str {
    if store.stands() { "update" } else { "build" }
}

/// How far the running refresh has got, for a reply that says one is already running.
fn progress(repo: &Path) -> String {
    status::progress(&status::log_tail(repo, "background.log"))
}

struct Args<'a>(&'a Value);

impl Args<'_> {
    fn optional_string(&self, key: &str) -> Option<String> {
        self.0.get(key).and_then(Value::as_str).map(str::to_string)
    }

    fn string(&self, key: &str) -> Result<String> {
        self.optional_string(key).filter(|s| !s.trim().is_empty()).with_context(|| format!("`{key}` is required and must be a non-empty string"))
    }

    fn flag(&self, key: &str) -> bool {
        self.0.get(key).and_then(Value::as_bool).unwrap_or(false)
    }

    /// A depth or a count, at least 1 as on the command line: 0 reaches nothing, and a `LOW` or a
    /// "no call path" would then read as an answer.
    fn number(&self, key: &str, default: usize) -> Result<usize> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(default),
            Some(v) => match v.as_u64() {
                Some(n) if n >= 1 => Ok(n as usize),
                _ => bail!("`{key}` must be a whole number of at least 1"),
            },
        }
    }
}

fn tools() -> Vec<Value> {
    let stale = json!({ "type": "boolean", "description": "Answer from the store as it stands, without bringing it in line with the tree first." });
    let depth = |what: &str, default: usize| json!({ "type": "integer", "minimum": 1, "description": format!("{what} (default {default}).") });
    let tool = |name: &str, description: &str, properties: Value, required: &[&str]| json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required },
    });
    vec![
        tool("ask", "What the docs and code say about a question, by meaning and not by grep. Names the retrievers' disagreement when they disagree.", json!({
            "question": { "type": "string" },
            "seeds": { "type": "integer", "minimum": 1, "description": "How many seeds the answer expands (default 5)." },
            "bodies": { "type": "boolean", "description": "Include the seeds' bodies." },
            "rerank": { "type": "boolean", "description": "Let the configured model command pick the seeds; costs tokens." },
            "rerank_local": { "type": "boolean", "description": "Pick the seeds with the local cross-encoder; slow cold, zero tokens." },
            "depth": depth("Candidates the reranker is shown", crate::rerank::DEPTH),
            "stale": stale,
        }), &["question"]),
        tool("explain", "One node: its text, its neighbours and where it is written.", json!({
            "node": { "type": "string", "description": "A node id or a name." }, "stale": stale,
        }), &["node"]),
        tool("impact", "Who reaches a symbol (callers by depth, importing files, a risk line), or with `down` what it reaches.", json!({
            "symbol": { "type": "string" },
            "depth": depth("Hops walked", 3),
            "down": { "type": "boolean", "description": "Walk what the symbol reaches instead of what reaches it." },
            "stale": stale,
        }), &["symbol"]),
        tool("trace", "The shortest chain of calls from one symbol to another, or that there is none within the depth.", json!({
            "from": { "type": "string" }, "to": { "type": "string" }, "depth": depth("Hops walked", 6), "stale": stale,
        }), &["from", "to"]),
        tool("changes", "What the working tree's diff touches and who reaches it.", json!({
            "base": { "type": "string", "description": "The revision the diff is taken against (default HEAD)." },
            "depth": depth("Caller hops", 2),
            "stale": stale,
        }), &[]),
        tool("status", "The index's state: size, files behind the tree, the model that wrote the vectors against the configured one, and whether a background refresh is running and how far it has got.", json!({}), &[]),
        tool("reindex", "Starts a background refresh of the index and returns at once: a first build where there is no index, else an update. Says so when one is already running. Follow it with `status`.", json!({}), &[]),
        tool("cache_status", "The machine's vector cache, which every checkout reads before it embeds: where it is, its cap, and the rows and bytes it holds per model. Writes nothing; a cache not made yet reads as empty.", json!({}), &[]),
        tool("switch_model", "Re-embeds the index under another embedding model, in the background. Only catalogued models and ones the user has trusted with `zerograph model <id>`; any other id is refused.", json!({
            "model": { "type": "string", "description": "A hub id." },
        }), &["model"]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "# A\n\n**FR-PAY-22 · MUST · cancellation window**\n\nA booking may be cancelled up to a day before.\n";
    const CODE: &str = "export function caller() { callee(); }\nexport function callee() { return 1; }\n";

    #[cfg(unix)]
    #[test]
    fn a_log_a_clone_linked_to_a_device_reads_as_no_output() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".zerograph")).unwrap();
        std::os::unix::fs::symlink("/dev/zero", dir.path().join(".zerograph/background.log")).unwrap();
        assert_eq!(progress(dir.path()), "no output yet");
    }

    /// A built store over one document and one source file, and a server that answers lexically.
    fn served() -> (tempfile::TempDir, Server) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("docs/a.md"), DOC).unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/x.ts"), CODE).unwrap();
        crate::run_update(dir.path(), &config::Config::default(), true).unwrap();
        let server = Server::new(dir.path(), true);
        (dir, server)
    }

    fn send(server: &Server, message: Value) -> Value {
        serde_json::from_str(&server.handle_line(&message.to_string()).expect("a request is answered")).unwrap()
    }

    fn call(server: &Server, name: &str, arguments: Value) -> (bool, String) {
        let reply = send(server, json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": name, "arguments": arguments } }));
        let r = &reply["result"];
        (r["isError"].as_bool().unwrap(), r["content"][0]["text"].as_str().unwrap().to_string())
    }

    #[test]
    fn initialize_names_the_protocol_the_server_and_its_tools_capability() {
        let (_dir, server) = served();
        let reply = send(&server, json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "0" } } }));
        assert_eq!(reply["id"], 1);
        assert_eq!(reply["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(reply["result"]["serverInfo"]["name"], "zerograph");
        assert!(reply["result"]["capabilities"]["tools"].is_object());
        let other = send(&server, json!({ "jsonrpc": "2.0", "id": 2, "method": "initialize", "params": { "protocolVersion": "1999-01-01" } }));
        assert_eq!(other["result"]["protocolVersion"], "2025-06-18", "a revision it does not speak is answered with the one it does");
    }

    #[test]
    fn tools_list_names_every_tool_with_a_schema() {
        let (_dir, server) = served();
        let reply = send(&server, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }));
        let names: Vec<&str> = reply["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["ask", "explain", "impact", "trace", "changes", "status", "reindex", "cache_status", "switch_model"]);
        for t in reply["result"]["tools"].as_array().unwrap() {
            assert_eq!(t["inputSchema"]["type"], "object", "{t}");
        }
    }

    #[test]
    fn a_notification_is_not_answered_and_a_ping_is() {
        let (_dir, server) = served();
        assert!(server.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        assert!(server.handle_line(r#"{"jsonrpc":"2.0","method":"no/such/notification"}"#).is_none());
        assert!(server.handle_line(r#"{"jsonrpc":"2.0","id":9,"result":{}}"#).is_none(), "a response is not a request");
        assert_eq!(send(&server, json!({ "jsonrpc": "2.0", "id": "p", "method": "ping" })), json!({ "jsonrpc": "2.0", "id": "p", "result": {} }));
    }

    #[test]
    fn an_unknown_method_is_32601_and_malformed_json_is_32700() {
        let (_dir, server) = served();
        let reply = send(&server, json!({ "jsonrpc": "2.0", "id": 3, "method": "resources/list" }));
        assert_eq!(reply["error"]["code"], -32601);
        assert_eq!(reply["id"], 3);
        let reply: Value = serde_json::from_str(&server.handle_line("{not json").unwrap()).unwrap();
        assert_eq!(reply["error"]["code"], -32700);
        assert_eq!(reply["id"], Value::Null);
        let reply: Value = serde_json::from_str(&server.handle_line("[1]").unwrap()).unwrap();
        assert_eq!(reply["error"]["code"], -32600, "batching is gone from this revision");
    }

    #[test]
    fn a_call_to_no_tool_is_a_protocol_error_and_a_failing_tool_is_a_result() {
        let (_dir, server) = served();
        let reply = send(&server, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "nope" } }));
        assert_eq!(reply["error"]["code"], -32602);
        let (is_error, text) = call(&server, "explain", json!({ "node": "no-such-node-anywhere" }));
        assert!(is_error, "{text}");
        let (is_error, text) = call(&server, "ask", json!({}));
        assert!(is_error && text.contains("question"), "{text}");
        let (is_error, text) = call(&server, "ask", json!({ "question": "a ".repeat(serve::HELLO_MAX) }));
        assert!(is_error && text.contains("the most one can be"), "{text}");
        let (is_error, _) = call(&server, "impact", json!({ "symbol": "callee", "depth": 0 }));
        assert!(is_error, "a depth of 0 reaches nothing");
    }

    #[test]
    fn paid_reranks_stop_at_the_allowance_and_say_so() {
        let (_dir, server) = served();
        let mut notices = Vec::new();
        assert!(server.may_rerank(2, &mut notices));
        server.reranks.fetch_add(2, std::sync::atomic::Ordering::SeqCst);
        assert!(notices.is_empty());
        assert!(!server.may_rerank(2, &mut notices), "two run is the whole of an allowance of two");
        assert!(notices[0].contains("mcp_reranks") && notices[0].contains("fused order"), "{notices:?}");
        assert!(!Server::new(Path::new("."), true).may_rerank(0, &mut Vec::new()), "0 turns rerank over MCP off");
    }

    /// The default command is refused when the project file names its own, so the rerank never
    /// runs — and a rerank that never ran is not one of the allowance's.
    #[test]
    fn a_rerank_that_did_not_run_is_not_counted() {
        let (dir, server) = served();
        std::fs::write(dir.path().join("zerograph.toml"), "rerank_command = \"cat\"\n").unwrap();
        let (is_error, text) = call(&server, "ask", json!({ "question": "FR-PAY-22 cancellation window", "stale": true, "rerank": true }));
        assert!(!is_error && text.contains("rerank:"), "{text}");
        assert_eq!(server.reranks.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn the_query_tools_answer_from_a_real_store() {
        let (_dir, server) = served();
        let (is_error, text) = call(&server, "ask", json!({ "question": "FR-PAY-22 cancellation window", "stale": true }));
        assert!(!is_error && text.contains("FR-PAY-22"), "{text}");
        let (is_error, text) = call(&server, "explain", json!({ "node": "FR-PAY-22" }));
        assert!(!is_error && text.contains("cancellation window"), "{text}");
        let (is_error, text) = call(&server, "impact", json!({ "symbol": "callee" }));
        assert!(!is_error && text.contains("caller"), "{text}");
        let (is_error, text) = call(&server, "trace", json!({ "from": "caller", "to": "callee" }));
        assert!(!is_error && text.contains("callee"), "{text}");
        let (is_error, text) = call(&server, "trace", json!({ "from": "callee", "to": "caller" }));
        assert!(!is_error && text.contains("no call path"), "no path is an answer: {text}");
    }

    #[test]
    fn changes_reports_the_working_diff() {
        let (dir, server) = served();
        let git = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(dir.path()).output().unwrap().status.success());
        git(&["init", "-q"]);
        git(&["add", "-A"]);
        git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "base", "--no-gpg-sign"]);
        std::fs::write(dir.path().join("src/x.ts"), "export function caller() { callee(); }\nexport function callee() { return 2; }\n").unwrap();
        let (is_error, text) = call(&server, "changes", json!({ "stale": true }));
        assert!(!is_error && text.contains("callee"), "{text}");
    }

    #[test]
    fn status_reports_the_store_and_a_running_refresh_only_while_it_runs() {
        let (dir, server) = served();
        let (is_error, text) = call(&server, "status", json!({}));
        assert!(!is_error, "{text}");
        assert!(text.contains("doc nodes") && text.contains("code nodes"), "{text}");
        assert!(text.contains("the index is in step"), "{text}");
        assert!(text.contains("refresh: none running"), "{text}");

        std::fs::write(dir.path().join("docs/b.md"), DOC).unwrap();
        let (_, text) = call(&server, "status", json!({}));
        assert!(text.contains("1 file behind") && text.contains("docs/b.md"), "{text}");

        let log = dir.path().join(".zerograph/background.log");
        std::fs::write(&log, "embedded 99/100 (1.0 rows/s, ~1 s left)\nrefresh: started\nembedded 16/40 (8.0 rows/s, ~3 s left)\nembedded 32/40 (8.0 rows/s, ~1 s left)\n").unwrap();
        let held = store::Store::new(dir.path()).try_lock_writer().unwrap().unwrap();
        let (_, text) = call(&server, "status", json!({}));
        assert!(text.contains("refresh: running (embedded 32/40"), "{text}");
        drop(held);
        let (_, text) = call(&server, "status", json!({}));
        assert!(text.contains("refresh: none running") && !text.contains("32/40"), "{text}");
    }

    #[cfg(unix)]
    #[test]
    fn a_spawned_refresh_is_running_before_it_holds_the_lock_and_is_reaped_after() {
        let (dir, server) = served();
        let child = std::process::Command::new("sleep").arg("1").spawn().unwrap();
        server.remember(refresh::reap(child));
        let (_, text) = call(&server, "status", json!({}));
        assert!(text.contains("refresh: running (starting)"), "{text}");
        let (is_error, text) = call(&server, "reindex", json!({}));
        assert!(!is_error && text.contains("already running"), "{text}");
        let (is_error, text) = call(&server, "switch_model", json!({ "model": index::embed::RECOMMENDED }));
        assert!(is_error || text.contains("already on"), "{text}");
        assert!(!dir.path().join(".zerograph/background.log").exists(), "a second spawn did not happen");
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while server.spawn_in_flight() && std::time::Instant::now() < until { std::thread::sleep(std::time::Duration::from_millis(50)); }
        let (_, text) = call(&server, "status", json!({}));
        assert!(text.contains("refresh: none running"), "{text}");
    }

    #[test]
    fn an_answer_carries_the_refresh_notes_the_terminal_would_have_seen() {
        let (dir, server) = served();
        std::fs::write(dir.path().join("docs/b.md"), DOC).unwrap();
        std::fs::write(dir.path().join("zerograph.toml"), "reader_budget = 0\n").unwrap();
        let _held = store::Store::new(dir.path()).try_lock_writer().unwrap().unwrap();
        let (is_error, text) = call(&server, "explain", json!({ "node": "FR-PAY-22" }));
        assert!(!is_error && text.contains("index: 1 file behind, a refresh is already running"), "{text}");
    }

    #[test]
    fn a_meter_from_before_the_current_run_started_is_not_shown() {
        let (dir, server) = served();
        std::fs::write(dir.path().join(".zerograph/background.log"), "embedded 99/100 (1.0 rows/s, ~1 s left)\n").unwrap();
        let _held = store::Store::new(dir.path()).try_lock_writer().unwrap().unwrap();
        let (_, text) = call(&server, "status", json!({}));
        assert!(text.contains("refresh: running (no output yet)"), "{text}");
    }

    #[test]
    fn reindex_says_so_when_a_refresh_holds_the_store_and_does_not_start_a_second() {
        let (dir, server) = served();
        let _held = store::Store::new(dir.path()).try_lock_writer().unwrap().unwrap();
        let (is_error, text) = call(&server, "reindex", json!({}));
        assert!(!is_error && text.contains("already running"), "{text}");
        assert!(!dir.path().join(".zerograph/background.log").exists(), "nothing was spawned to write a log");
    }

    #[test]
    fn reindex_builds_where_no_store_stands_and_updates_where_one_does() {
        let bare = tempfile::tempdir().unwrap();
        assert_eq!(refresh_command(&store::Store::new(bare.path())), "build");
        let (dir, _server) = served();
        assert_eq!(refresh_command(&store::Store::new(dir.path())), "update");
    }

    #[test]
    fn cache_status_is_the_report_the_command_line_prints() {
        let (_dir, server) = served();
        let (is_error, text) = call(&server, "cache_status", json!({}));
        let printed = crate::cache_command(crate::CacheAction::Status).map_err(|e| format!("{e:#}"));
        match printed {
            Ok(printed) => assert!(!is_error && text == crate::terminal_safe(&printed), "{text}"),
            Err(err) => assert!(is_error && text == err, "{text}"),
        }
    }

    #[test]
    fn switch_model_refuses_an_id_nobody_catalogued_or_trusted() {
        let (dir, server) = served();
        let (is_error, text) = call(&server, "switch_model", json!({ "model": "attacker/never-heard-of-this-model" }));
        assert!(is_error, "{text}");
        assert!(text.contains("zerograph model attacker/never-heard-of-this-model"), "the refusal says how a person trusts it: {text}");
        assert!(!dir.path().join(config::PROJECT_FILE).exists(), "nothing was written");
        let (is_error, _) = call(&server, "switch_model", json!({ "model": "  " }));
        assert!(is_error, "a blank id is not trusted by a blank line in the trust file");
    }
}
