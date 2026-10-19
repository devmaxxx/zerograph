//! What the index is and what is being done to it, for a person (`zerograph status`) and for an
//! agent (the MCP `status` tool) alike: one report, so the two never say different things about
//! the same store. It reads the store, its logs and the serve socket, and writes nothing — not
//! even the store directory, which a writer lock would otherwise create on a project not built.

use crate::{config, index, prime, refresh, serve, store, walk};
use anyhow::Result;
use serde::Serialize;
use std::io::{Read, Seek};
use std::path::Path;
use std::time::Duration;

/// How much of a log's end is read. A refresh prints a line every two seconds, and the log is
/// kept across runs up to a megabyte, so the end is all that is current.
const LOG_TAIL: u64 = 16 * 1024;

/// Lines of a run's own output shown under its refresh line. Meters are left out, so these are
/// what the run said besides how far it had got: its result, and any warning on the way.
const RUN_LINES: usize = 10;

/// Lines of the resident server's log shown under its line, from its last start.
const SERVE_LINES: usize = 5;

#[derive(Serialize)]
pub(crate) struct Report {
    pub index: Index,
    pub tree: Tree,
    pub model: Model,
    /// `None` when no vectors were written.
    pub vectors: Option<Vectors>,
    pub refresh: Refresh,
    /// The refresh log's lines after its last start marker, without meters.
    pub last_run: Vec<String>,
    pub serve: Serve,
    /// A newer release the daily check found, read from its cache and never fetched here.
    pub update: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum Index {
    Empty,
    Built { doc_nodes: usize, code_nodes: usize, edges: usize },
    /// Reported rather than raised: a damaged store is when the rest of this report is wanted most.
    Unreadable { error: String },
}

#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum Tree {
    InStep,
    /// The store was written by an older grammar, and the next read or refresh re-reads every file.
    /// `release` is empty for a store written before releases were recorded.
    OtherGrammar { grammar: u32, release: String },
    /// A newer release wrote the store: this build answers from it as it stands and never
    /// rewrites it, so how far the tree has moved is that release's to find out.
    NewerGrammar { grammar: u32, release: String },
    Behind { files: Vec<String> },
    NotWalked { error: String },
    /// The configuration did not load, so the walk it steers could not run.
    ConfigUnread { error: String },
}

#[derive(Serialize)]
pub(crate) struct Model {
    /// The model that wrote the vectors.
    pub recorded: Option<String>,
    /// The model `zerograph.toml` names; `None` when the configuration did not load.
    pub configured: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum Vectors {
    Rows { rows: usize, owed: usize },
    Unreadable { error: String },
}

#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum Refresh {
    None,
    /// Spawned by this process and not yet holding the writer lock.
    Starting,
    Running { progress: String },
    Unknown { error: String },
}

#[derive(Serialize)]
pub(crate) struct Serve {
    pub running: bool,
    /// The socket checked, which is the one `serve` binds: a reader that looks for a server on its
    /// own takes the path from here rather than computing the name a second time.
    pub socket: String,
    /// Seconds since the socket was bound, while one answers.
    pub up_secs: Option<u64>,
    /// `serve.log` from its last start, at most `SERVE_LINES` lines.
    pub log: Vec<String>,
}

/// The report as it stands. `starting` is a caller's own knowledge that it spawned a refresh
/// which has not taken the lock yet.
pub(crate) fn report(repo: &Path, starting: bool) -> Result<Report> {
    let store = store::Store::new(repo);
    let (graph, manifest, index) = match store.load() {
        Ok((graph, _)) if graph.nodes.is_empty() => (graph, Default::default(), Index::Empty),
        Ok((graph, manifest)) => {
            let brief = prime::brief(&graph, 0, None);
            let index = Index::Built { doc_nodes: brief.docs, code_nodes: brief.code, edges: brief.edges };
            (graph, manifest, index)
        }
        Err(err) => (Default::default(), Default::default(), Index::Unreadable { error: format!("{err:#}") }),
    };
    let cfg = config::Config::load(repo);
    // Before the walk, which runs on the global pool: the first call fixes its size for good.
    if let Ok(cfg) = &cfg { crate::cap_pools(index::embed::threads(cfg.resources)); }
    let tree = match &cfg {
        // Before the walk: this build never refreshes such a store, so how far the tree moved is
        // not its to report.
        _ if manifest.newer_grammar() => Tree::NewerGrammar { grammar: manifest.grammar, release: manifest.release.clone() },
        Ok(cfg) => match walk::walk(repo, cfg, &manifest) {
            Ok(entries) => match manifest.diff(&entries) {
                // No hash reports a store an older grammar wrote, but a read treats it as behind
                // and re-reads every file, so "in step" would contradict the next answer.
                d if d.changed.is_empty() && d.removed.is_empty() && manifest.stale_grammar() => Tree::OtherGrammar { grammar: manifest.grammar, release: manifest.release.clone() },
                d if d.changed.is_empty() && d.removed.is_empty() => Tree::InStep,
                d => Tree::Behind { files: refresh::files_of(&d) },
            },
            Err(err) => Tree::NotWalked { error: format!("{err:#}") },
        },
        Err(err) => Tree::ConfigUnread { error: format!("{err:#}") },
    };
    let (recorded, vectors) = match index::dense::DenseIndex::recorded_model(&store) {
        Err(err) => (None, Some(Vectors::Unreadable { error: format!("{err:#}") })),
        Ok(recorded) => {
            let vectors = vectors(&store, &graph, cfg.as_ref().ok(), recorded.is_some());
            (recorded, vectors)
        }
    };
    let model = Model { recorded, configured: cfg.as_ref().ok().map(|c| c.embed_model.clone()) };
    let log = log_tail(repo, "background.log");
    Ok(Report { index, tree, model, vectors, refresh: refresh_state(repo, starting, &log), last_run: last_run(&log), serve: serve_state(repo), update: crate::update::known() })
}

fn vectors(store: &store::Store, graph: &crate::model::Graph, cfg: Option<&config::Config>, recorded: bool) -> Option<Vectors> {
    match recorded && index::dense::DenseIndex::present(store) {
        false => None,
        true => Some(match index::dense::DenseIndex::load(store) {
            Ok(dense) => {
                let rows = dense.ids.iter().filter(|id| !id.is_empty()).count();
                let skip = cfg.and_then(|c| index::dense::EmbedSkip::new(&c.embed_skip).ok()).unwrap_or_default();
                Vectors::Rows { rows, owed: dense.owed(graph, &skip) }
            }
            Err(err) => Vectors::Unreadable { error: format!("{err:#}") },
        }),
    }
}

/// Whether the writer lock is held cannot go stale: a killed process releases it with its
/// handle, where a pid file would outlive it. The question is asked by taking the lock and
/// dropping it at once — the one probe the lock has — so a writer that tries for it in that
/// instant waits (`update`) or answers from the store as it stands (a reader's refresh), as it
/// would behind any other writer. A store directory that does not exist has no writer, and
/// asking the lock would create it.
fn refresh_state(repo: &Path, starting: bool, log: &[String]) -> Refresh {
    if !store::dir_of(repo).is_dir() {
        return match starting { true => Refresh::Starting, false => Refresh::None };
    }
    match (store::Store::new(repo).try_lock_writer(), starting) {
        (Ok(Some(_)), false) => Refresh::None,
        (Ok(Some(_)), true) => Refresh::Starting,
        (Ok(None), _) => Refresh::Running { progress: progress(log) },
        (Err(err), _) => Refresh::Unknown { error: format!("{err:#}") },
    }
}

fn by_release(release: &str) -> String {
    match release.is_empty() { true => String::new(), false => format!(" (zerograph {release})") }
}

/// Only the lines after the last start marker are this run's: the log is kept across runs, and
/// a run that was killed leaves a meter behind that no later line takes back.
fn current_run(log: &[String]) -> Option<&[String]> {
    log.iter().rposition(|l| l == refresh::RUN_MARKER).map(|started| &log[started + 1..])
}

fn is_meter(line: &str) -> bool { line.starts_with("embedded ") }

/// How far the running refresh has got: its latest meter, else its latest line.
pub(crate) fn progress(log: &[String]) -> String {
    let Some(current) = current_run(log) else { return "no output yet".to_string() };
    match (current.iter().rev().find(|l| is_meter(l)), current.last()) {
        (Some(meter), _) => meter.to_string(),
        (None, Some(last)) => last.to_string(),
        (None, None) => "no output yet".to_string(),
    }
}

fn last_run(log: &[String]) -> Vec<String> {
    let lines: Vec<String> = current_run(log).unwrap_or_default().iter().filter(|l| !is_meter(l)).cloned().collect();
    lines[lines.len().saturating_sub(RUN_LINES)..].to_vec()
}

fn serve_state(repo: &Path) -> Serve {
    let path = serve::socket_path(repo);
    let running = serve::answering(&path);
    // The socket is bound once, at start, and never touched again, so its age is the server's.
    let up_secs = running.then(|| std::fs::symlink_metadata(&path).and_then(|m| m.modified()).ok()?.elapsed().ok().map(|d| d.as_secs())).flatten();
    let socket = path.display().to_string();
    let log = log_tail(repo, "serve.log");
    // Lines from before a start the tail no longer holds would be read as this server's.
    let Some(since) = log.iter().rposition(|l| serve::is_start_line(l)) else { return Serve { running, socket, up_secs, log: vec![] } };
    let tail = &log[since..];
    Serve { running, socket, up_secs, log: tail[tail.len().saturating_sub(SERVE_LINES)..].to_vec() }
}

/// The last `LOG_TAIL` bytes of a store log, one entry per line. A meter redraws itself with a
/// carriage return, so that splits a line too.
pub(crate) fn log_tail(repo: &Path, name: &str) -> Vec<String> {
    let read = || -> Result<String> {
        let Some(mut f) = store::open_for_read(&store::dir_of(repo).join(name))? else { return Ok(String::new()) };
        let len = f.metadata()?.len();
        f.seek(std::io::SeekFrom::Start(len.saturating_sub(LOG_TAIL)))?;
        let mut bytes = Vec::new();
        f.take(LOG_TAIL).read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    read().unwrap_or_default().split(['\n', '\r']).map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

/// The report as lines. `build` is how the reader builds an empty index: a person runs a
/// command, an agent calls a tool.
pub(crate) fn render(r: &Report, build: &str) -> String {
    let mut out = String::new();
    let mut say = |line: String| { out.push_str(&line); out.push('\n'); };
    say(match &r.index {
        Index::Empty => format!("index: empty; {build} builds it"),
        Index::Built { doc_nodes, code_nodes, edges } => format!("index: {doc_nodes} doc nodes, {code_nodes} code nodes, {edges} edges"),
        Index::Unreadable { error } => format!("index: unreadable ({error}); {build} rewrites it"),
    });
    say(match &r.tree {
        Tree::OtherGrammar { grammar, release } => format!("tree: the index was written by grammar {grammar}{} and is re-read whole by the next read or `reindex`", by_release(release)),
        Tree::NewerGrammar { grammar, release } => format!("tree: the index was written by grammar {grammar}{}, newer than this build's {} ({}); answered read-only, never rewritten — a newer zerograph refreshes it",
            by_release(release), walk::GRAMMAR, env!("CARGO_PKG_VERSION")),
        Tree::InStep => "tree: the index is in step with it".to_string(),
        Tree::Behind { files } => format!("tree: {} ({})", refresh::files_line(files.len()), preview(files)),
        Tree::NotWalked { error } => format!("tree: not walked ({error})"),
        Tree::ConfigUnread { error } => format!("config: not read ({error})"),
    });
    say(match (&r.model.recorded, &r.model.configured) {
        (Some(m), Some(c)) if m.eq_ignore_ascii_case(c) => format!("model: {m} wrote the vectors and is the configured one"),
        (Some(m), Some(c)) => format!("model: {m} wrote the vectors; {c} is configured, and takes effect when `reindex` or `switch_model` rewrites them"),
        (Some(m), None) => format!("model: {m} wrote the vectors"),
        (None, Some(c)) => format!("model: no vectors yet; {c} is configured"),
        (None, None) => "model: no vectors yet".to_string(),
    });
    match &r.vectors {
        Some(Vectors::Rows { rows, owed }) => say(format!("vectors: {rows} rows, {owed} owed")),
        Some(Vectors::Unreadable { error }) => say(format!("vectors: unreadable ({error})")),
        None => {}
    }
    say(match &r.refresh {
        Refresh::None => "refresh: none running".to_string(),
        Refresh::Starting => "refresh: running (starting)".to_string(),
        Refresh::Running { progress } => format!("refresh: running ({progress})"),
        Refresh::Unknown { error } => format!("refresh: unknown ({error})"),
    });
    for line in &r.last_run { say(format!("  {line}")); }
    say(match (r.serve.running, r.serve.up_secs) {
        (true, Some(s)) => format!("serve: answering, up {} ({})", duration(s), r.serve.socket),
        (true, None) => format!("serve: answering ({})", r.serve.socket),
        (false, _) => format!("serve: not running (nothing answers at {})", r.serve.socket),
    });
    if !r.serve.log.is_empty() {
        let log = format!("{}/serve.log", store::DIR);
        match r.serve.running {
            true => say(format!("  last lines of {log}:")),
            false => say(format!("  last lines of {log}, from the server that ran last:")),
        }
        for line in &r.serve.log { say(format!("    {line}")); }
    }
    if let Some(latest) = &r.update {
        let (npm, pnpm) = crate::update::commands(latest);
        say(format!("update: zerograph {latest} is out and this is {}; `{npm}` or `{pnpm}`", env!("CARGO_PKG_VERSION")));
    }
    out
}

fn duration(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
    }
}

fn preview(files: &[String]) -> String {
    const SHOWN: usize = 5;
    let more = files.len().saturating_sub(SHOWN);
    let shown = files.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", ");
    match more {
        0 => shown,
        n => format!("{shown}, and {n} more"),
    }
}

/// How often `--follow` reads the lock and the log; a meter comes every two seconds.
const FOLLOW_EVERY: Duration = Duration::from_millis(500);

/// How long `--follow` waits for a refresh to take the lock when none holds it yet: a detached
/// `update` started just before it is a process start away from the lock.
const START_GRACE: Duration = Duration::from_secs(3);

/// Prints the refresh line each time it moves on from `shown` — the line the report already
/// printed — until the running refresh lets go of the store, then how it ended. A run that let go
/// without its finished line was killed or failed, and fails this command too, so a script that
/// waits on it learns how it ended.
pub(crate) fn follow(repo: &Path, shown: &Refresh) -> Result<()> {
    let running = || store::dir_of(repo).is_dir() && matches!(store::Store::new(repo).try_lock_writer(), Ok(None));
    let grace = std::time::Instant::now() + START_GRACE;
    while !running() {
        if std::time::Instant::now() >= grace { return Ok(()); }
        std::thread::sleep(FOLLOW_EVERY);
    }
    let mut shown = match shown { Refresh::Running { progress } => progress.clone(), _ => String::new() };
    while running() {
        let now = progress(&log_tail(repo, "background.log"));
        if now != shown {
            println!("refresh: running ({now})");
            shown = now;
        }
        std::thread::sleep(FOLLOW_EVERY);
    }
    let log = log_tail(repo, "background.log");
    let run = current_run(&log).unwrap_or_default();
    if run.last().is_some_and(|l| l == refresh::RUN_FINISHED) {
        println!("refresh: finished");
        return Ok(());
    }
    let last = run.iter().rev().find(|l| !is_meter(l)).or(run.last()).map_or("no output", String::as_str);
    anyhow::bail!("the refresh ended without finishing; its last line: {last}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> { s.lines().map(str::to_string).collect() }

    #[test]
    fn a_run_shows_what_it_said_besides_its_meters_and_nothing_from_the_run_before() {
        let log = lines("dense: index unreadable, from the killed run\nrefresh: started\nrefresh: 3 changed, 0 removed\nembedded 16/40 (8.0 rows/s, ~3 s left)\ndense: index unreadable, continuing lexical-only (bad header)\nembedded 32/40 (8.0 rows/s, ~1 s left)");
        assert_eq!(progress(&log), "embedded 32/40 (8.0 rows/s, ~1 s left)");
        assert_eq!(last_run(&log), ["refresh: 3 changed, 0 removed", "dense: index unreadable, continuing lexical-only (bad header)"]);
    }

    #[test]
    fn a_log_with_no_start_marker_has_no_current_run() {
        let log = lines("embedded 99/100 (1.0 rows/s, ~1 s left)");
        assert_eq!(progress(&log), "no output yet");
        assert!(last_run(&log).is_empty());
    }

    #[test]
    fn a_long_run_keeps_only_its_latest_lines() {
        let log: Vec<String> = std::iter::once(refresh::RUN_MARKER.to_string()).chain((0..25).map(|i| format!("line {i}"))).collect();
        let shown = last_run(&log);
        assert_eq!(shown.len(), RUN_LINES);
        assert_eq!(shown.last().unwrap(), "line 24");
    }

    #[test]
    fn a_project_never_built_reports_without_creating_its_store() {
        let dir = tempfile::tempdir().unwrap();
        let r = report(dir.path(), false).unwrap();
        assert!(matches!(r.index, Index::Empty) && matches!(r.refresh, Refresh::None) && !r.serve.running);
        let text = render(&r, "`zerograph build`");
        assert!(text.starts_with("index: empty; `zerograph build` builds it\n"), "{text}");
        assert!(text.contains(&format!("serve: not running (nothing answers at {})\n", serve::socket_path(dir.path()).display())),
                "the report names the socket it checked: {text}");
        assert!(!store::dir_of(dir.path()).exists(), "status wrote the store directory");
    }

    #[test]
    fn a_store_a_newer_release_wrote_is_named_with_its_grammar_and_release() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("docs/a.md"), "# A\n\n**FR-PAY-1 · MUST · refund**\n\nbody\n").unwrap();
        crate::run_update(dir.path(), &config::Config::default(), true).unwrap();
        let store = store::Store::new(dir.path());
        let (graph, manifest) = store.load().unwrap();
        store.save(&graph, &walk::Manifest { grammar: walk::GRAMMAR + 1, release: "9.9.9".into(), ..manifest }).unwrap();
        // A tree that moved since is still not "behind" for this build, which will not refresh it.
        std::fs::write(dir.path().join("docs/b.md"), "# B\n").unwrap();
        let r = report(dir.path(), false).unwrap();
        assert!(matches!(&r.tree, Tree::NewerGrammar { grammar, release } if *grammar == walk::GRAMMAR + 1 && release == "9.9.9"));
        let text = render(&r, "`build`");
        assert!(text.contains(&format!("grammar {} (zerograph 9.9.9), newer than this build's {}", walk::GRAMMAR + 1, walk::GRAMMAR)), "{text}");
        assert_eq!(serde_json::to_value(&r).unwrap()["tree"]["state"], "newer_grammar");
    }

    #[test]
    fn a_damaged_store_is_reported_with_everything_else_still_said() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(store::dir_of(dir.path())).unwrap();
        std::fs::write(store::dir_of(dir.path()).join("graph.json"), "{ not json").unwrap();
        std::fs::write(store::dir_of(dir.path()).join("background.log"), "refresh: started\nchanged 1 removed 0 nodes 2 edges 3\n").unwrap();
        let r = report(dir.path(), false).unwrap();
        assert!(matches!(r.index, Index::Unreadable { .. }), "{}", render(&r, "`build`"));
        assert_eq!(r.last_run, ["changed 1 removed 0 nodes 2 edges 3"]);
    }

    #[test]
    fn serve_lines_from_before_a_start_the_tail_lost_are_not_shown() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(store::dir_of(dir.path())).unwrap();
        std::fs::write(store::dir_of(dir.path()).join("serve.log"), "serve: dropped the model\n").unwrap();
        assert!(serve_state(dir.path()).log.is_empty());
    }

    #[test]
    fn the_json_names_each_state_by_a_tag() {
        let dir = tempfile::tempdir().unwrap();
        let v = serde_json::to_value(report(dir.path(), false).unwrap()).unwrap();
        assert_eq!(v["refresh"]["state"], "none");
        assert_eq!(v["serve"]["running"], false);
        assert_eq!(v["serve"]["socket"], serve::socket_path(dir.path()).display().to_string(), "the socket `serve` binds is the one checked");
        assert_eq!(v["index"]["state"], "empty");
        assert!(v["last_run"].as_array().unwrap().is_empty());
        for key in ["index", "tree", "model", "vectors", "refresh", "last_run", "serve"] {
            assert!(v.get(key).is_some(), "missing {key}: {v}");
        }
    }

    /// Holds the writer lock as a refresh would, writes `ends_on` as the run's last line, and lets
    /// go after a moment; returns what `follow` made of it.
    fn followed(ends_on: &str) -> Result<()> {
        let dir = tempfile::tempdir().unwrap();
        let held = store::Store::new(dir.path()).try_lock_writer().unwrap().unwrap();
        let log = store::dir_of(dir.path()).join("background.log");
        std::fs::write(&log, "refresh: started\nembedded 1/2 (1.0 rows/s, ~1 s left)\n").unwrap();
        let ends_on = ends_on.to_string();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(800));
            std::fs::write(&log, format!("refresh: started\nembedded 1/2 (1.0 rows/s, ~1 s left)\n{ends_on}\n")).unwrap();
            drop(held);
        });
        let followed = follow(dir.path(), &Refresh::None);
        releaser.join().unwrap();
        followed
    }

    #[test]
    fn follow_waits_for_the_running_refresh_and_fails_with_it() {
        followed(refresh::RUN_FINISHED).unwrap();
        let err = followed("Error: the store went away").unwrap_err();
        assert!(format!("{err:#}").contains("the store went away"), "{err:#}");
        let killed = followed("embedded 2/2 (1.0 rows/s, ~0 s left)").unwrap_err();
        assert!(format!("{killed:#}").contains("ended without finishing"), "a run killed on a meter is not done: {killed:#}");
    }

    #[test]
    fn follow_returns_at_once_when_nothing_runs() {
        let dir = tempfile::tempdir().unwrap();
        follow(dir.path(), &Refresh::None).unwrap();
        assert!(!store::dir_of(dir.path()).exists());
    }
}
