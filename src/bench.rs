use crate::config::Config;
use crate::index::dense::DenseIndex;
use crate::index::embed::Embedder;
use crate::model::{Graph, NodeKind};
use crate::query::{self, Answer, Options};
use crate::store::Store;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::path::Path;

/// What a case expects the answer to reach: one node id or file path, or several when the
/// honest answer to the question is more than one place.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Expect { One(String), Many(Vec<String>) }

impl Expect {
    pub fn anchors(&self) -> &[String] {
        match self { Expect::One(a) => std::slice::from_ref(a), Expect::Many(v) => v }
    }

    /// The per-case line and the history key: anchors joined by `+`, a character neither an
    /// id nor a path contains.
    pub fn key(&self) -> String { self.anchors().join("+") }
}

impl From<&str> for Expect {
    fn from(a: &str) -> Self { Expect::One(a.to_string()) }
}

#[derive(Debug, Deserialize)]
pub struct Case { pub kind: String, pub q: String, pub expect: Expect }

/// Hits per kind, in the order the case file introduces the kinds, so the summary line reads
/// in the order a person wrote the file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub by_kind: Vec<(String, (usize, usize))>,
    /// Anchors reached over anchors wanted, per kind — printed beside the counts and graded by
    /// nothing. A floor here is for the first reading that has a baseline to set one from; ADR-001
    /// is why this ships as a column and not as a bar.
    pub anchors: Vec<(String, (usize, usize))>,
    pub p90_tokens: usize,
}

impl Summary {
    #[cfg(test)]
    pub fn recorded(keyword: (usize, usize), paraphrase: (usize, usize), code: (usize, usize), p90_tokens: usize) -> Summary {
        let by_kind = vec![("keyword".to_string(), keyword), ("paraphrase".to_string(), paraphrase), ("code".to_string(), code)];
        Summary { by_kind, anchors: Vec::new(), p90_tokens }
    }

    /// `(hits, cases)` for one kind; a kind the file never named reads `(0, 0)`.
    pub fn kind(&self, kind: &str) -> (usize, usize) {
        self.by_kind.iter().find(|(k, _)| k == kind).map(|(_, c)| *c).unwrap_or((0, 0))
    }

    #[cfg(test)]
    pub fn with(mut self, kind: &str, count: (usize, usize)) -> Summary {
        *self.slot(kind) = count;
        self
    }

    fn slot(&mut self, kind: &str) -> &mut (usize, usize) {
        if let Some(i) = self.by_kind.iter().position(|(k, _)| k == kind) { return &mut self.by_kind[i].1; }
        self.by_kind.push((kind.to_string(), (0, 0)));
        &mut self.by_kind.last_mut().unwrap().1
    }

    /// One case's verdict and its completeness. The two are counted apart on purpose: a case that
    /// keeps its verdict and loses two of its three anchors moves nothing a count of cases can
    /// see, and one did — `multi` read `HIT 3/3` before a change every clause of its rule called
    /// identical and `HIT 1/3` after, in both arms.
    fn record(&mut self, kind: &str, hit: bool, anchors: (usize, usize)) {
        let slot = self.slot(kind);
        slot.1 += 1;
        if hit { slot.0 += 1; }
        let a = self.anchor_slot(kind);
        a.0 += anchors.0;
        a.1 += anchors.1;
    }

    /// `(reached, wanted)` anchors for one kind, summed over its cases.
    pub fn anchors(&self, kind: &str) -> (usize, usize) {
        self.anchors.iter().find(|(k, _)| k == kind).map(|(_, c)| *c).unwrap_or((0, 0))
    }

    fn anchor_slot(&mut self, kind: &str) -> &mut (usize, usize) {
        if let Some(i) = self.anchors.iter().position(|(k, _)| k == kind) { return &mut self.anchors[i].1; }
        self.anchors.push((kind.to_string(), (0, 0)));
        &mut self.anchors.last_mut().unwrap().1
    }
}

/// How many of the case's anchors the answer reaches, over how many it has. An anchor that
/// names a node counts anywhere in the answer, seeds or expansion; a file path counts only
/// among the seeds, as `code` cases always have — the expansion names neighbours, and a file
/// reached only as the neighbour of the wrong seed is not a file the developer was pointed at.
/// `is_id` is the graph's knowledge of which strings are node ids, so the case file never has
/// to say which shape an anchor has.
pub fn found(case: &Case, answer: &Answer, is_id: &dyn Fn(&str) -> bool) -> (usize, usize) {
    let anchors = case.expect.anchors();
    let reached = anchors.iter().filter(|a| {
        if is_id(a) {
            answer.seeds.iter().chain(answer.expanded.iter()).any(|h| &h.id == *a)
        } else {
            answer.seeds.iter().any(|h| &h.file == *a)
        }
    }).count();
    (reached, anchors.len())
}

/// One anchor reached is a hit: the developer has an entry point. `found` says how complete it was.
pub fn hit(case: &Case, answer: &Answer, is_id: &dyn Fn(&str) -> bool) -> bool {
    found(case, answer, is_id).0 >= 1
}

/// Which embedder's floors a dense arm is graded against. The lexical arms have no embedder and
/// read one set whatever the store's rows were written by; a dense arm under a model with no
/// floors of its own is measured and not graded, the way another case file is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Floors { Small, Gemma, None }

/// Private on purpose: `dense_grading` is the only way to reach the floors, so a caller cannot
/// key them off the store's recorded rows while the embedder that answers is a resolved override.
fn floors_for(model: Option<&str>) -> Floors {
    match model {
        None => Floors::Small,
        Some(m) if m == crate::index::embed::UNNAMED_MODEL => Floors::Small,
        Some(m) if m == crate::index::embed::DEFAULT_MODEL || m == crate::index::embed::GEMMA => Floors::Gemma,
        Some(_) => Floors::None,
    }
}

/// Floors and the printed `model=` field for one run, keyed to the same model throughout: the
/// resolved dense model when a dense arm runs, else the store's recorded rows (the only model a
/// lexical-only run has). Factored out of `run` so the override case — `resolved` naming a model
/// the store was never written by — is reachable by a test without opening one.
fn dense_grading(no_dense: bool, recorded: Option<&str>, resolved: Option<&str>) -> (Floors, String) {
    let model = if no_dense { recorded } else { resolved };
    let floors = floors_for(model);
    let field = match floors {
        Floors::Small => "small".to_string(),
        Floors::Gemma => "gemma".to_string(),
        Floors::None => model.unwrap_or_default().to_string(),
    };
    (floors, field)
}

/// `(dense, floors) → (keyword, paraphrase)`. Every number is one the recorded cases measured,
/// never a target: gemma's dense arm sits one paraphrase under the 19/30 it read twice, and the
/// small model's pair is flush (the paragraphs in `passes`). The lexical row carries
/// `Floors::Small` and is read for every model: no embedder is in it. `bench/history/track.py`
/// reads this table out of the source; keep the rows one per line.
const FLOORS: [(bool, Floors, usize, usize); 3] = [
    (true, Floors::Gemma, 40, 18),
    (true, Floors::Small, 40, 9),
    (false, Floors::Small, 39, 7),
];

const P90_CEILING: usize = 250;

/// Whether an arm has floors to be graded against: a lexical one always does, a dense one only
/// where its model measured that arm.
fn graded(dense: bool, floors: Floors) -> bool {
    !dense || FLOORS.iter().any(|r| r.0 && r.1 == floors)
}

pub fn passes(s: &Summary, dense: bool, floors: Floors) -> bool {
    // Every floor is the number the recorded cases measure; only the token ceiling is rounded.
    // It was 230, over the small model's four p90s (220 to 226), until a requirement's answer gained
    // the line naming one symbol that cites it: that moved the p90 to 236 with every count unchanged,
    // and 250 holds it with the share every default language adds (238 and 250 on the fixture).
    // A count equal to its total is an exact floor: `run` grades against these only when the case
    // file has the recorded 40/30/12 shape.
    //
    // Paraphrase is the split the arms disagree on most: the small model measures 9 with
    // embeddings and 7 without — three of its questions are reached by the dense passage list and
    // by neither lexical list.
    //
    // Keyword is 39 in the lexical-only arm, not 40: `FR-PH-43` sits at passage rank 23 and no
    // lexical path reaches it.
    //
    // The small model's pair has no headroom, unlike gemma's: 9 and 7 are runs sitting flush on
    // the noisiest split. If they flap, they are the first thing to relax.
    //
    // A dense arm has no floors of its own under a model that never measured that arm — grading
    // it would be inventing a bar. `run` still prints what it found; it just cannot say pass or fail.
    if dense && floors == Floors::None { return false; }
    let key = if dense { floors } else { Floors::Small };
    let Some(&(_, _, keyword, paraphrase)) = FLOORS.iter().find(|r| r.0 == dense && r.1 == key) else { return false };
    s.kind("keyword").0 >= keyword && s.kind("paraphrase").0 >= paraphrase && s.kind("code").0 >= 12 && s.p90_tokens <= P90_CEILING
}

// The recorded cases travel inside the binary so a release build benches from any directory.
const BUILT_IN_CASES: &str = include_str!("../bench/cases.jsonl");

/// The corpus the recorded cases and the floors were written against, as `<corpus> <commit>`.
/// The cases name paths, and a path is a fact about one checkout: when the corpus renames a
/// directory the case file has not drifted from the truth, it has drifted from *that* pin, and a
/// reader comparing a run against the floors needs to know which tree the floors were counted on.
const BUILT_IN_PIN: &str = include_str!("../bench/cases.pin");

/// The shape the floors were measured on. A file of this shape is graded; any other file is
/// measured and reported, and its exit code claims nothing.
const RECORDED_SHAPE: [(&str, usize); 3] = [("keyword", 40), ("paraphrase", 30), ("code", 12)];

fn parse_cases(text: &str) -> Result<Vec<Case>> {
    text.lines().filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Case>(l).map_err(anyhow::Error::from))
        .collect()
}

/// Kinds in order of first appearance, each with its count.
fn shape(cases: &[Case]) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for c in cases {
        match out.iter_mut().find(|(k, _)| *k == c.kind) {
            Some((_, n)) => *n += 1,
            None => out.push((c.kind.clone(), 1)),
        }
    }
    out
}

fn is_recorded_shape(cases: &[Case]) -> bool {
    let mut got = shape(cases);
    got.sort();
    let mut want: Vec<(String, usize)> = RECORDED_SHAPE.iter().map(|(k, n)| (k.to_string(), *n)).collect();
    want.sort();
    got == want
}

/// The checkout's commit when it is a different tree from the pin's, which the floors were never
/// counted on: a corpus that grew 203 documents past the pin read paraphrase 12 against its 15,
/// and grading that as a failure would make every consumer's bench red for the corpus's own
/// growth. A tree whose commit cannot be read is graded as it always was.
fn off_pin(pin: &str, head: Option<&str>) -> Option<String> {
    let commit = pin.split_whitespace().nth(1)?;
    let head = head?.trim();
    (!head.is_empty() && !head.starts_with(commit)).then(|| head.chars().take(commit.len()).collect())
}

fn files_of(graph: &Graph) -> HashSet<&str> {
    graph.nodes.values().map(|n| n.file.as_str()).collect()
}

/// A `code` case asks where one symbol is defined, and the case's `q` is that symbol, so a
/// corpus that moves the file has not changed the question, only where its answer lives. Such an
/// anchor is re-pointed at the one file that defines the name now, and each move is returned for
/// the transcript; a name defined in several files, or in none, is left for `check_anchors` to
/// refuse.
fn relocate_moved(cases: &mut [Case], graph: &Graph, files: &HashSet<&str>) -> Vec<String> {
    let mut moves = Vec::new();
    for case in cases.iter_mut().filter(|c| c.kind == "code") {
        let Expect::One(anchor) = &mut case.expect else { continue };
        if graph.nodes.contains_key(anchor.as_str()) || files.contains(anchor.as_str()) { continue; }
        let tail = format!("::{}", case.q);
        let mut defined: BTreeSet<&str> = graph.nodes.values()
            .filter(|n| n.kind == NodeKind::Symbol && (n.label == case.q || n.id.ends_with(&tail)))
            .map(|n| n.file.as_str()).collect();
        if defined.len() == 1 {
            let file = defined.pop_first().expect("checked len == 1 above");
            moves.push(format!("{:?} moved: {anchor} -> {file}", case.q));
            *anchor = file.to_string();
        }
    }
    moves
}

/// Every anchor must be a node id or a file some node declares. A mistyped anchor would
/// otherwise score as a miss for as long as nobody read the transcript, and a weak spot that is
/// really a typo is the one kind this suite must not report. Every such anchor is named in the one
/// error: a corpus that moves a directory strands several cases at once, and stopping at the first
/// costs a run per anchor to find the rest.
fn check_anchors(cases_path: &str, pin: Option<&str>, cases: &[Case], graph: &Graph, files: &HashSet<&str>) -> Result<()> {
    let stale: Vec<String> = cases.iter()
        .flat_map(|c| c.expect.anchors().iter().map(move |a| (c, a)))
        .filter(|(_, a)| !graph.nodes.contains_key(*a) && !files.contains(a.as_str()))
        .map(|(c, a)| format!("{:?} expects {a:?}", c.q))
        .collect();
    let against = pin.map(|p| format!("\nThe cases are recorded against {p}; `bench/missing-anchors.py <checkout>` lists the path anchors a tree is missing.")).unwrap_or_default();
    match stale.as_slice() {
        [] => Ok(()),
        [one] => anyhow::bail!("{cases_path}: {one}, which is neither a node id nor a file any node declares{against}"),
        all => anyhow::bail!("{cases_path}: {} anchors are neither a node id nor a file any node declares:\n  {}{against}", all.len(), all.join("\n  ")),
    }
}

/// What one reranked question was shown: the pool in the order it was ranked in, and the size of
/// the prompt that carried it. Empty on a case that never reranked — an exact id match answers
/// before the pool is built, and a case that never reached the model has no rank to report.
#[derive(Default)]
struct Shown { pool: Vec<String>, prompt_bytes: Option<usize> }

impl Shown {
    /// The command arm: a prompt is built and sent, so it is metered by building the same string a
    /// second time rather than by estimating it from the candidate count — an estimate dressed as
    /// a measurement is what this line exists to replace.
    fn sent(q: &str, c: &[(String, String)]) -> Shown {
        Shown { pool: ids(c), prompt_bytes: Some(crate::rerank::prompt(q, c).len()) }
    }

    /// The local arm: the same pool, scored pair by pair through ONNX. No prompt is sent, so none
    /// is reported — a byte count for a request nobody made would be a fabricated column beside a
    /// measured one.
    fn scored(c: &[(String, String)]) -> Shown {
        Shown { pool: ids(c), prompt_bytes: None }
    }

    fn clear(&mut self) { self.pool.clear(); self.prompt_bytes = None; }

    /// The diagnostic beneath a case line, or nothing where the model was never asked. The rank is
    /// of the first anchor found; a case whose anchor is outside the pool reads `-`, which is the
    /// answer to whether depth or fusion is the lever for it.
    fn case_line(&self, case: &Case) -> Option<String> {
        if self.pool.is_empty() { return None; }
        let rank = case.expect.anchors().iter().find_map(|a| pool_rank(&self.pool, a));
        let rank = rank.map_or("-".to_string(), |r| r.to_string());
        let prompt = self.prompt_bytes.map_or(String::new(), |b| format!(" prompt={b} B"));
        Some(format!("{:<10} pool={rank}/{}{prompt}", "", self.pool.len()))
    }
}

fn ids(c: &[(String, String)]) -> Vec<String> { c.iter().map(|(id, _)| id.clone()).collect() }

/// Where the expected id sits in the pool the reranker was shown, one-based. A case the model
/// found at rank 6 is one a sixth seat could seat for nothing; a case at 140 is the model's alone,
/// and a fusion change judged against it is a change judged against the model.
fn pool_rank(pool: &[String], want: &str) -> Option<usize> {
    pool.iter().position(|id| id == want).map(|i| i + 1)
}

/// Median and p90 of the metered prompts, in bytes, on a line of its own. Bytes and not tokens
/// because bytes are what this process can count: the token figure belongs to whichever model the
/// command runs, and an estimate dressed as a measurement is what this line exists to replace.
fn prompt_line(bytes: &[usize]) -> Option<String> {
    if bytes.is_empty() { return None; }
    let mut b = bytes.to_vec();
    b.sort_unstable();
    let median = b[b.len() / 2];
    let p90 = b[b.len() * 9 / 10];
    Some(format!("rerank prompt: median {median} B  p90 {p90} B  n={} (bytes, not tokens)", b.len()))
}

pub fn run(repo: &Path, cases: Option<&Path>, no_dense: bool, rerank: bool, rerank_local: bool, depth: usize) -> Result<(bool, Summary)> {
    // Resolved before `Config::load` so the override repo's own `zerograph.toml` — not the
    // `--repo` one — is the file this run reads.
    let repo = crate::env_os("BENCH_REPO").map(std::path::PathBuf::from).unwrap_or(repo.to_path_buf());
    let cfg = Config::load(&repo)?;
    if rerank { if let Some(why) = cfg.refusal("rerank_command") { anyhow::bail!(why); } }
    // `bench` does not come through `main`'s arms, so the pools it owns are capped here.
    let threads = crate::index::embed::threads(cfg.resources);
    crate::cap_pools(threads);
    let store = Store::new(&repo);
    let (graph, _): (Graph, _) = store.load()?;
    if graph.nodes.is_empty() { anyhow::bail!("graph is empty at {} — run build first", repo.display()); }
    let counted = crate::families::of_graph(&graph);
    let family_count = counted.0.len() + counted.1.len();
    let dense_idx = DenseIndex::load(&store)?;
    let recorded = DenseIndex::recorded_model(&store)?;
    let lex = crate::index::lexical::Lexical::build(&graph);
    // `ask` degrading to lexical-only on a missing model is fine — a person reading the answer
    // sees the stderr notice and can judge it. `bench` speaks only through its exit code, so a
    // dense run that silently falls back and then grades against the weaker no-dense floor
    // would report green without ever having checked what it claims to check.
    //
    // Resolved before the embedder opens, so the floors and the printed model name key off the
    // model that actually answers the queries — `ZEROGRAPH_EMBED_MODEL` outranks the store's
    // recorded rows here exactly as it does when `Embedder::open` reads it below. A store's own
    // width says nothing about which model wrote it: an override of that width is still a
    // foreign model, and must be graded (and reported) as itself, not borrow the store's floors.
    // A dense bench grades the dense arm, so a store whose model is turned away fails it rather
    // than grade the lexical arm against dense floors.
    let resolved = match no_dense {
        true => None,
        false => Some(crate::index::embed::reader_model(recorded.as_deref(), &cfg.embed_model).map_err(|notice| anyhow::anyhow!("{notice}"))?),
    };
    let (floors, model_field) = dense_grading(no_dense, recorded.as_deref(), resolved.as_deref());
    let mut embedder = match &resolved {
        None => None,
        Some(model) => match Embedder::open(model, threads, crate::index::embed::Weights::Packed) {
            Ok(e) => Some(e),
            Err(err) => anyhow::bail!("dense: model unavailable ({err:#})"),
        },
    };
    if !no_dense && dense_idx.ids.is_empty() {
        anyhow::bail!("dense index is empty at {} — run `zerograph update` first", repo.display());
    }
    if let Some(e) = embedder.as_mut() {
        // A bench that silently searched 384-d queries against 1024-d rows would read empty
        // dense lists as a lexical-only run and grade it against the wrong floors.
        let width = e.dim()?;
        if dense_idx.dim > 0 && width != dense_idx.dim {
            anyhow::bail!("{}", crate::index::embed::width_mismatch(dense_idx.dim, e.name(), width));
        }
    }
    let dense_on = !no_dense;
    let embedder = std::cell::RefCell::new(embedder);
    let dense_fn = |q: &str, k: usize| -> Vec<String> {
        let mut e = embedder.borrow_mut();
        match e.as_mut().and_then(|e| e.query(q).ok()) { Some(v) => dense_idx.search(&v, k), None => Vec::new() }
    };
    let built_in = cases.is_none();
    let (cases_path, suite, text) = match cases {
        Some(p) => {
            let suite = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string());
            (p.display().to_string(), suite, std::fs::read_to_string(p).with_context(|| p.display().to_string())?)
        }
        None => ("built-in bench/cases.jsonl".to_string(), "built-in".to_string(), BUILT_IN_CASES.to_string()),
    };
    let mut cases: Vec<Case> = parse_cases(&text)?;
    if cases.is_empty() { anyhow::bail!("{cases_path} holds no cases"); }
    // The recorded case set is a constant, not an input: a truncated or edited copy of it would
    // otherwise still pass, since every floor is relative to whatever total showed up. Another
    // file is another suite — measured, printed, never graded against floors it did not earn.
    let shape_ok = is_recorded_shape(&cases);
    if built_in && !shape_ok {
        let got = shape(&cases).iter().map(|(k, n)| format!("{n} {k}")).collect::<Vec<_>>().join(" / ");
        anyhow::bail!("{cases_path} has {got} cases, expected 40 keyword / 30 paraphrase / 12 code");
    }
    // A dense arm under a model with no floors of its own is still worth running — the case
    // file's shape earned grading, the store's embedder just never measured any. `gated` says so
    // through the exit code rather than a bail, so the run still prints what it found.
    let mut gated = shape_ok && graded(dense_on, floors);
    // Named before the cases run, not after: a reader comparing the counts below against the
    // README floors is comparing them against this tree.
    let pin = built_in.then(|| BUILT_IN_PIN.trim()).filter(|p| !p.is_empty());
    if let Some(p) = pin {
        println!("suite: {cases_path}, recorded against {p}");
        let head = crate::changes::git(&repo, &["rev-parse", "HEAD"]).ok();
        if let Some(head) = off_pin(p, head.as_deref()) {
            println!("suite: checkout {head} is not the pin, so this run is measured and not graded");
            gated = false;
        }
    }
    let files = files_of(&graph);
    for moved in relocate_moved(&mut cases, &graph, &files) { println!("suite: {moved}"); }
    check_anchors(&cases_path, pin, &cases, &graph, &files)?;
    let is_id = |a: &str| graph.nodes.contains_key(a);
    let opts = Options { seeds: 5, bodies: false, dense: dense_on, json: false, depth };
    // What the reranked arm was actually shown, kept for the case line below: the pool it ranked
    // and the size of the prompt that carried it. Both are properties of the request, so they are
    // read where the request is made rather than reconstructed from the answer afterwards.
    let shown = std::cell::RefCell::new(Shown::default());
    let rerank_fn = |q: &str, c: &[(String, String)]| {
        *shown.borrow_mut() = Shown::sent(q, c);
        crate::rerank::run(&cfg.rerank_command, q, c)
    };
    let cross = std::cell::RefCell::new(if rerank_local {
        let dir = if cfg.reranker_dir.is_empty() { crate::index::cross::default_dir()? } else { std::path::PathBuf::from(&cfg.reranker_dir) };
        Some(crate::index::cross::CrossEncoder::open(&dir, threads).context("--rerank-local")?)
    } else { None });
    let local_fn = |q: &str, c: &[(String, String)]| -> Vec<String> {
        *shown.borrow_mut() = Shown::scored(c);
        let mut m = cross.borrow_mut();
        let Some(m) = m.as_mut() else { return Vec::new() };
        let texts: Vec<String> = c.iter().map(|(_, t)| t.clone()).collect();
        match m.score(q, &texts) {
            Ok(s) => crate::index::cross::pick(&s, c, crate::index::cross::PICK),
            // Like a failing rerank command: say so and answer from the fused order.
            Err(e) => { eprintln!("rerank-local: {e:#}; answering from the fused order"); Vec::new() }
        }
    };
    let rerank: Option<query::Rerank> = if rerank_local { Some(&local_fn) } else if rerank { Some(&rerank_fn) } else { None };
    let mut summary = Summary::default();
    let mut tokens = Vec::new();
    let mut prompts = Vec::new();
    for case in &cases {
        let words: Vec<String> = case.q.split_whitespace().map(str::to_string).collect();
        shown.borrow_mut().clear();
        let answer = query::ask(&graph, &lex, Some(&dense_fn), rerank, &words, &opts);
        let rendered = query::render(&answer, &graph, &opts);
        let tok = rendered.len() / 4;
        tokens.push(tok);
        let (reached, want) = found(case, &answer, &is_id);
        let ok = hit(case, &answer, &is_id);
        summary.record(&case.kind, ok, (reached, want));
        println!("{:<10} {:<12} {} {reached}/{want} {:>4} tok  {}", case.kind, case.expect.key(), if ok { "HIT " } else { "miss" }, tok, case.q);
        // Indented, and on a line of its own, for the reason `anchor_line` gives: `track.py`'s
        // case pattern reads the question as everything after the token count, so a column added
        // there would be recorded as part of the question in every transcript from here on.
        if let Some(line) = shown.borrow().case_line(case) {
            println!("{line}");
            prompts.extend(shown.borrow().prompt_bytes);
        }
    }
    tokens.sort_unstable();
    summary.p90_tokens = tokens.get(tokens.len() * 9 / 10).copied().unwrap_or(0);
    let counts = summary.by_kind.iter().map(|(k, (h, t))| format!("{k} {h}/{t}")).collect::<Vec<_>>().join("  ");
    println!("\n{counts}  p90 {} tok  dense={dense_on}  model={model_field} families={}{}  suite={suite} gated={gated}",
        summary.p90_tokens, family_count, match (rerank_local, rerank.is_some()) {
            (true, _) => format!(" rerank_local=true depth={depth}"),
            (false, true) => format!(" rerank=true depth={depth}"),
            _ => String::new(),
        });
    // A line of its own, and beneath the summary rather than inside it: `bench/history/track.py`
    // parses that line with a regex of `<kind> <hits>/<cases>` pairs, and every transcript the
    // campaign has recorded is read back through it. A column added there would have made this
    // measurement retire the history it exists to extend.
    println!("{}", anchor_line(&summary));
    if let Some(line) = prompt_line(&prompts) { println!("{line}"); }
    // The ceiling bounds what the zero-token arm prints. A reranked answer's size is whichever
    // seeds the model picked, and the same store read 248 and 259 on two haiku runs, so a reranked
    // run is graded on its counts alone.
    let reranked = rerank.is_some();
    let ok = passes(&summary, dense_on, floors)
        || reranked && passes(&Summary { p90_tokens: P90_CEILING, ..summary.clone() }, dense_on, floors);
    Ok((!gated || ok, summary))
}

/// Anchors reached over anchors wanted, per kind, in the summary's own order.
pub fn anchor_line(s: &Summary) -> String {
    let kinds = s.by_kind.iter().map(|(k, _)| { let (r, w) = s.anchors(k); format!("{k} {r}/{w}") }).collect::<Vec<_>>().join("  ");
    format!("anchors  {kinds}")
}

/// The median of `n` runs of the same suite, per kind and for the token p90. G23: the reader bars
/// this suite is judged against are tighter than its own repeatability — max RSS bounced 1.36 to
/// 1.56 GB on *both* binaries of a control — so a reading worth comparing is a median of several,
/// not a single run.
pub fn median(runs: &[Summary]) -> Summary {
    let mid = |mut v: Vec<usize>| -> usize { v.sort_unstable(); v.get(v.len() / 2).copied().unwrap_or(0) };
    let mut out = Summary { p90_tokens: mid(runs.iter().map(|r| r.p90_tokens).collect()), ..Summary::default() };
    for (kind, _) in runs.first().map(|r| r.by_kind.clone()).unwrap_or_default() {
        *out.slot(&kind) = (
            mid(runs.iter().map(|r| r.kind(&kind).0).collect()),
            mid(runs.iter().map(|r| r.kind(&kind).1).collect()),
        );
        *out.anchor_slot(&kind) = (
            mid(runs.iter().map(|r| r.anchors(&kind).0).collect()),
            mid(runs.iter().map(|r| r.anchors(&kind).1).collect()),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::Hit;

    /// The rank of the expected id inside the pool the reranker was shown, one-based, because a
    /// histogram of "rank 1" and a histogram of "rank 0" read differently to everyone but a
    /// programmer. A case whose anchor is outside the pool has no rank, and that is the finding.
    #[test]
    fn the_pool_rank_of_the_expected_id_is_recorded() {
        let pool = vec!["FR-CAL-1".to_string(), "FR-MKT-35".to_string(), "FR-PAY-22".to_string()];
        assert_eq!(pool_rank(&pool, "FR-MKT-35"), Some(2));
        assert_eq!(pool_rank(&pool, "FR-SVC-50"), None);
    }

    /// The diagnostic is printed for a case that reranked and for no other: a question answered by
    /// an exact id match never builds a pool, and a `-` there would read as "outside the pool".
    #[test]
    fn the_case_line_appears_only_where_the_model_was_asked() {
        let case = Case { kind: "paraphrase".into(), q: "отмена".into(), expect: Expect::from("FR-MKT-35") };
        assert_eq!(Shown::default().case_line(&case), None, "no pool, no line");

        let candidates: Vec<(String, String)> = ["FR-CAL-1", "FR-MKT-35"].iter()
            .map(|id| ((*id).to_string(), "текст".to_string())).collect();
        let shown = Shown::sent("отмена", &candidates);
        let line = shown.case_line(&case).unwrap();
        assert!(line.contains("pool=2/2"), "{line}");
        let bytes = shown.prompt_bytes.expect("the command arm sends a prompt");
        assert!(line.contains(&format!("prompt={bytes} B")), "{line}");
        assert!(bytes > 0, "the prompt is metered, not estimated");

        // The local arm scores the same pool through ONNX and sends no prompt, so it reports none:
        // a byte count for a request nobody made would be a fabricated column beside a measured one.
        let local = Shown::scored(&candidates);
        let line = local.case_line(&case).unwrap();
        assert!(line.contains("pool=2/2"), "{line}");
        assert!(!line.contains("prompt="), "{line}");

        let missing = Case { kind: "paraphrase".into(), q: "отмена".into(), expect: Expect::from("FR-SVC-50") };
        assert!(shown.case_line(&missing).unwrap().contains("pool=-/2"), "outside the pool is not rank zero");
    }

    /// Median and p90 over the metered prompts, and nothing at all when no case reranked — the
    /// line is a measurement of a run that spent a model, not a zero to be averaged into history.
    #[test]
    fn the_prompt_line_reports_bytes_and_says_they_are_bytes() {
        assert_eq!(prompt_line(&[]), None);
        let line = prompt_line(&[100, 200, 300, 400, 500, 600, 700, 800, 900, 1000]).unwrap();
        assert!(line.contains("median 600 B"), "{line}");
        assert!(line.contains("p90 1000 B"), "{line}");
        assert!(line.contains("n=10"), "{line}");
        assert!(line.contains("bytes, not tokens"), "{line}");
    }

    /// A case that keeps its verdict and loses two of its three anchors is what the summary was
    /// blind to. The counts and the anchors are two different readings of the same run, and only
    /// the second one moves here.
    #[test]
    fn the_summary_carries_anchors_reached_over_wanted() {
        let mut s = Summary::default();
        s.record("multi", true, (3, 3));
        s.record("multi", true, (1, 3));
        s.record("keyword", false, (0, 1));
        assert_eq!(s.kind("multi"), (2, 2), "both cases are hits");
        assert_eq!(s.anchors("multi"), (4, 6), "and four of six anchors were reached");
        assert_eq!(s.anchors("keyword"), (0, 1));
        assert_eq!(s.anchors("nothing-of-the-sort"), (0, 0));
    }

    fn h(id: &str, file: &str) -> Hit { Hit { id: id.into(), file: file.into(), line: 1, label: String::new(), score: 1.0, via: None } }

    fn case(kind: &str, expect: Expect) -> Case { Case { kind: kind.into(), q: String::new(), expect } }

    fn many(anchors: &[&str]) -> Expect { Expect::Many(anchors.iter().map(|a| a.to_string()).collect()) }

    // Node ids in these fixtures never contain a slash; paths always do.
    fn is_id(a: &str) -> bool { !a.contains('/') }

    #[test]
    fn hit_rules() {
        let a = Answer { seeds: vec![h("sym:x.ts::f", "packages/x.ts")], expanded: vec![h("FR-PAY-22", "d.md")] };
        assert!(hit(&case("paraphrase", "FR-PAY-22".into()), &a, &is_id));
        assert!(hit(&case("code", "packages/x.ts".into()), &a, &is_id));
        assert!(!hit(&case("keyword", "FR-PAY-23".into()), &a, &is_id));

        // A substring of a real id is not a match: ids compare whole-string, not `contains`.
        assert!(!hit(&case("keyword", "PAY-22".into()), &a, &is_id));
        // Ids compare verbatim, not case-folded.
        assert!(!hit(&case("keyword", "fr-pay-22".into()), &a, &is_id));

        // A code case's expected path is only a suffix of the hit's file: `file` compares
        // whole-string, not `ends_with`.
        let nested = Answer { seeds: vec![h("sym:packages/nested/x.ts::f", "packages/nested/x.ts")], expanded: vec![] };
        assert!(!hit(&case("code", "x.ts".into()), &nested, &is_id));
        // A path only reachable through `expanded` does not count: paths score `seeds` only.
        let expanded_only = Answer { seeds: vec![], expanded: vec![h("sym:packages/x.ts::f", "packages/x.ts")] };
        assert!(!hit(&case("code", "packages/x.ts".into()), &expanded_only, &is_id));
    }

    #[test]
    fn an_anchor_matches_on_the_axis_its_shape_names_and_never_on_the_other() {
        // The hit's id and its file are both present; an id anchor must not be satisfied by the
        // file and a path anchor must not be satisfied by the id, or a case would score on a
        // coincidence between two unrelated strings.
        let a = Answer { seeds: vec![h("packages/x.ts", "FR-PAY-22")], expanded: vec![] };
        assert!(!hit(&case("keyword", "FR-PAY-22".into()), &a, &is_id), "an id anchor read off the file field");
        assert!(!hit(&case("code", "packages/x.ts".into()), &a, &is_id), "a path anchor read off the id field");
    }

    #[test]
    fn a_case_with_several_anchors_reports_how_many_it_reached() {
        let a = Answer { seeds: vec![h("FR-PAY-20", "p.md"), h("sym:x.ts::f", "packages/x.ts")], expanded: vec![h("FR-PAY-22", "d.md")] };
        let c = case("multi", many(&["FR-PAY-20", "FR-PAY-22", "FR-PAY-99"]));
        assert_eq!(found(&c, &a, &is_id), (2, 3));
        assert!(hit(&c, &a, &is_id), "one anchor reached is an entry point");

        // Mixed shapes: the id counts from the expansion, the path only from the seeds.
        let cross = case("cross", many(&["FR-PAY-22", "packages/x.ts"]));
        assert_eq!(found(&cross, &a, &is_id), (2, 2));
        let expanded_path = Answer { seeds: vec![], expanded: vec![h("FR-PAY-22", "d.md"), h("sym:x.ts::f", "packages/x.ts")] };
        assert_eq!(found(&cross, &expanded_path, &is_id), (1, 2));

        // Nothing reached is a miss, whatever the anchor count.
        assert_eq!(found(&case("multi", many(&["FR-A-1", "FR-A-2"])), &a, &is_id), (0, 2));
        assert!(!hit(&case("multi", many(&["FR-A-1", "FR-A-2"])), &a, &is_id));
    }

    #[test]
    fn the_graph_decides_which_anchors_are_ids() {
        // A task id carries a slash and would otherwise read as a path; the graph knows better.
        let a = Answer { seeds: vec![], expanded: vec![h("BE-M01/T01", "docs/m.md")] };
        let graph_says_id = |x: &str| x == "BE-M01/T01";
        assert!(hit(&case("keyword", "BE-M01/T01".into()), &a, &graph_says_id));
        assert!(!hit(&case("keyword", "BE-M01/T01".into()), &a, &is_id), "as a path it is looked for among seed files");
    }

    #[test]
    fn expect_parses_as_one_string_or_a_list() {
        let one: Case = serde_json::from_str(r#"{"kind":"keyword","q":"a","expect":"FR-X-1"}"#).unwrap();
        assert_eq!(one.expect.anchors(), ["FR-X-1"]);
        assert_eq!(one.expect.key(), "FR-X-1");
        let several: Case = serde_json::from_str(r#"{"kind":"multi","q":"a","expect":["FR-X-1","packages/x.ts"]}"#).unwrap();
        assert_eq!(several.expect.anchors(), ["FR-X-1", "packages/x.ts"]);
        assert_eq!(several.expect.key(), "FR-X-1+packages/x.ts");
        assert!(serde_json::from_str::<Case>(r#"{"kind":"multi","q":"a","expect":7}"#).is_err());
    }

    #[test]
    fn only_the_recorded_shape_is_gated() {
        let mut recorded = Vec::new();
        for (kind, n) in RECORDED_SHAPE { for _ in 0..n { recorded.push(case(kind, "X".into())); } }
        assert!(is_recorded_shape(&recorded));
        // Order of kinds in the file does not matter, the counts do.
        recorded.reverse();
        assert!(is_recorded_shape(&recorded));

        let mut short = recorded.iter().map(|c| case(&c.kind, "X".into())).collect::<Vec<_>>();
        short.pop();
        assert!(!is_recorded_shape(&short), "one case short of the recorded shape");
        let mut extra_kind = recorded.iter().map(|c| case(&c.kind, "X".into())).collect::<Vec<_>>();
        extra_kind.push(case("long", "X".into()));
        assert!(!is_recorded_shape(&extra_kind), "a fourth kind is another suite");
        assert!(!is_recorded_shape(&[case("long", "X".into()), case("cross", "X".into())]));
    }

    #[test]
    fn the_shape_keeps_the_order_the_file_introduced() {
        let cases = [case("long", "X".into()), case("cross", "X".into()), case("long", "X".into())];
        assert_eq!(shape(&cases), [("long".to_string(), 2), ("cross".to_string(), 1)]);
    }

    #[test]
    fn a_summary_counts_kinds_in_file_order_and_reads_absent_kinds_as_zero() {
        let s = Summary::default().with("long", (3, 5)).with("cross", (1, 2));
        assert_eq!(s.by_kind, [("long".to_string(), (3, 5)), ("cross".to_string(), (1, 2))]);
        assert_eq!(s.kind("cross"), (1, 2));
        assert_eq!(s.kind("keyword"), (0, 0));
        // `recorded` is the three graded kinds in their fixed order.
        assert_eq!(Summary::recorded((40, 40), (14, 30), (12, 12), 220).kind("paraphrase"), (14, 30));
    }

    #[test]
    fn floors() {
        // Fixtures sit exactly on each floor so a boundary shifted by one in either direction
        // reddens the corresponding call; a fixture comfortably clear of the floor (the
        // original mistake) would not notice such a shift.
        let at_floor_nodense = Summary::recorded((39, 40), (7, 30), (12, 12), 230);
        let at_floor_dense = Summary::recorded((40, 40), (9, 30), (12, 12), 230);
        let at_floor_gemma = Summary::recorded((40, 40), (18, 30), (12, 12), 230);
        assert!(passes(&at_floor_nodense, false, Floors::Small));
        assert!(passes(&at_floor_dense, true, Floors::Small));
        assert!(passes(&at_floor_gemma, true, Floors::Gemma));

        // keyword: one short of its floor reddens either arm — 38 without embeddings, 39 with.
        assert!(!passes(&at_floor_nodense.clone().with("keyword", (38, 40)), false, Floors::Small));
        assert!(!passes(&at_floor_dense.clone().with("keyword", (39, 40)), true, Floors::Small));
        assert!(!passes(&at_floor_gemma.clone().with("keyword", (39, 40)), true, Floors::Gemma));

        // code must be exact: one short reddens every arm.
        assert!(!passes(&at_floor_nodense.clone().with("code", (11, 12)), false, Floors::Small));
        assert!(!passes(&at_floor_dense.clone().with("code", (11, 12)), true, Floors::Small));
        assert!(!passes(&at_floor_gemma.clone().with("code", (11, 12)), true, Floors::Gemma));

        // p90: one token over the shared ceiling reddens either arm.
        assert!(!passes(&Summary { p90_tokens: P90_CEILING + 1, ..at_floor_nodense.clone() }, false, Floors::Small));
        assert!(!passes(&Summary { p90_tokens: P90_CEILING + 1, ..at_floor_dense.clone() }, true, Floors::Small));

        // paraphrase: one short of each arm's floor reddens it.
        assert!(!passes(&at_floor_nodense.clone().with("paraphrase", (6, 30)), false, Floors::Small));
        assert!(!passes(&at_floor_dense.clone().with("paraphrase", (8, 30)), true, Floors::Small));
        assert!(!passes(&at_floor_gemma.clone().with("paraphrase", (17, 30)), true, Floors::Gemma));
        // The small model's dense run does not clear gemma's bar: the floors follow the model.
        assert!(!passes(&at_floor_dense, true, Floors::Gemma));

        // A summary of another suite has none of the graded kinds and reads as every floor
        // missed, which is why `run` never grades one.
        assert!(!passes(&Summary::default().with("long", (15, 15)), true, Floors::Small));

        // A model with no floors of its own is measured and never graded in its dense arm; its
        // lexical arm has no embedder in it and reads the small model's floors.
        assert!(!passes(&at_floor_dense, true, Floors::None));
        assert!(passes(&at_floor_nodense, false, Floors::None), "lexical arms do not read the model");
        assert!(!passes(&at_floor_nodense.clone().with("keyword", (38, 40)), false, Floors::None));
    }

    #[test]
    fn the_floors_follow_the_model_the_rows_were_written_by() {
        use crate::index::embed::UNNAMED_MODEL;
        assert_eq!(floors_for(None), Floors::Small, "a store with no vectors is graded lexically, on floors the model never enters");
        assert_eq!(floors_for(Some(UNNAMED_MODEL)), Floors::Small);
        assert_eq!(floors_for(Some("BAAI/bge-m3")), Floors::None);
        // Whichever model the default names, a store a first build wrote has to be graded and not
        // merely measured: a default moved to a model with no floors of its own would turn every
        // fresh store's `bench` green-by-abstention, `gated=false` with nothing red to say so.
        assert_ne!(floors_for(Some(crate::index::embed::DEFAULT_MODEL)), Floors::None);
        assert!(graded(true, floors_for(Some(crate::index::embed::DEFAULT_MODEL))));
    }

    #[test]
    fn an_arm_its_model_never_measured_is_measured_and_not_graded() {
        assert!(graded(false, Floors::None), "a lexical arm reads no model");
        assert!(graded(true, Floors::Small));
        assert!(graded(true, Floors::Gemma));
        assert!(!graded(true, Floors::None));
    }

    #[test]
    fn dense_grading_follows_the_resolved_model_not_the_stores_recorded_one() {
        // `ZEROGRAPH_EMBED_MODEL` pointed at a foreign model of the store's own width: the width
        // guard upstream would pass, but this model was never measured, so it must read
        // `Floors::None` and print its own name — never the small model's floors it happens to
        // share a dimension with.
        let (floors, field) = dense_grading(false, Some(crate::index::embed::UNNAMED_MODEL), Some("BAAI/bge-small-en-v1.5"));
        assert_eq!(floors, Floors::None);
        assert_eq!(field, "BAAI/bge-small-en-v1.5");

        // No override: the resolved model is what `resolve` would already have returned for
        // this store, so grading is unchanged.
        let (floors, field) = dense_grading(false, Some(crate::index::embed::UNNAMED_MODEL), Some(crate::index::embed::UNNAMED_MODEL));
        assert_eq!(floors, Floors::Small);
        assert_eq!(field, "small");

        // `--no-dense`: there is no embedder to resolve, so grading stays keyed to the store's
        // own recorded rows regardless of what `resolved` would have been.
        let (floors, field) = dense_grading(true, Some(crate::index::embed::UNNAMED_MODEL), Some("BAAI/bge-small-en-v1.5"));
        assert_eq!(floors, Floors::Small);
        assert_eq!(field, "small");
    }

    #[test]
    fn cases_file_parses_and_has_the_recorded_shape() {
        let cases = parse_cases(BUILT_IN_CASES).unwrap();
        assert!(is_recorded_shape(&cases));
        assert!(cases.iter().all(|c| c.expect.anchors().len() == 1), "every recorded case expects one place");
    }

    /// The recorded cases against a corpus on disk: every anchor is still a node or a declared file
    /// there. `bench` reads the store the same way and checks this before its first question, so a
    /// corpus that moved a file is found here without the model and the 82 questions behind it.
    #[test]
    #[ignore = "needs a built corpus: set ZEROGRAPH_BENCH_REPO to the beauty-crm checkout to check"]
    fn every_built_in_anchor_is_in_the_corpus() {
        let repo = crate::env("BENCH_REPO").expect("ZEROGRAPH_BENCH_REPO");
        let (graph, _): (Graph, _) = Store::new(Path::new(&repo)).load().unwrap();
        assert!(!graph.nodes.is_empty(), "no graph at {repo}: run `zerograph build` there first");
        check_anchors("built-in bench/cases.jsonl", Some(BUILT_IN_PIN.trim()), &parse_cases(BUILT_IN_CASES).unwrap(), &graph, &files_of(&graph)).unwrap();
    }

    #[test]
    fn keyword_hit_counts_a_seed_or_an_expanded_entry() {
        let a = Answer { seeds: vec![h("FR-PAY-22", "d.md")], expanded: vec![h("FR-PAY-20", "e.md")] };
        assert!(hit(&case("keyword", "FR-PAY-22".into()), &a, &is_id));
        assert!(hit(&case("keyword", "FR-PAY-20".into()), &a, &is_id));
    }

    #[test]
    fn paraphrase_hit_counts_a_seed_or_an_expanded_entry() {
        let a = Answer { seeds: vec![h("FR-PAY-22", "d.md")], expanded: vec![h("FR-PAY-20", "e.md")] };
        assert!(hit(&case("paraphrase", "FR-PAY-22".into()), &a, &is_id));
        assert!(hit(&case("paraphrase", "FR-PAY-20".into()), &a, &is_id));
    }

    #[test]
    fn code_hit_only_counts_a_seed_never_an_expanded_entry() {
        let a = Answer { seeds: vec![h("sym:x.ts::f", "packages/x.ts")], expanded: vec![h("sym:y.ts::g", "packages/y.ts")] };
        assert!(hit(&case("code", "packages/x.ts".into()), &a, &is_id));
        assert!(!hit(&case("code", "packages/y.ts".into()), &a, &is_id));
    }

    #[test]
    fn anchors_are_checked_against_the_graph_before_anything_is_asked() {
        use crate::model::{Node, NodeKind};
        let mut graph = Graph::default();
        let node = |id: &str, file: &str| Node {
            id: id.into(), kind: NodeKind::Requirement, label: String::new(), body: String::new(), file: file.into(), line: 1, end: 0,
            files: ["docs/y.md".to_string()].into_iter().collect(), community: None,
        };
        graph.nodes.insert("FR-X-1".into(), node("FR-X-1", "docs/x.md"));
        graph.nodes.insert("sym:a".into(), node("sym:a", "packages/a.ts"));
        let good = [case("cross", many(&["FR-X-1", "packages/a.ts"])), case("long", "FR-X-1".into())];
        assert!(check_anchors("f", None, &good, &graph, &files_of(&graph)).is_ok());
        let bad_id = [case("long", "FR-X-2".into())];
        let err = check_anchors("f", None, &bad_id, &graph, &files_of(&graph)).unwrap_err().to_string();
        assert!(err.contains("FR-X-2"), "{err}");
        let bad_path = [case("where", "packages/b.ts".into())];
        assert!(check_anchors("f", None, &bad_path, &graph, &files_of(&graph)).is_err());
        // A file some node lists as a secondary location is not the file a hit reports.
        let also_listed = [case("where", "docs/y.md".into())];
        assert!(check_anchors("f", None, &also_listed, &graph, &files_of(&graph)).is_err());
    }

    /// A corpus that moves a directory strands every case under it at once. Naming the first and
    /// stopping costs a run per anchor to learn the rest, each behind a full store load.
    #[test]
    fn every_stale_anchor_is_named_in_one_error() {
        use crate::model::{Node, NodeKind};
        let mut graph = Graph::default();
        graph.nodes.insert("FR-X-1".into(), Node {
            id: "FR-X-1".into(), kind: NodeKind::Requirement, label: String::new(), body: String::new(), file: "docs/x.md".into(), line: 1, end: 0,
            files: Default::default(), community: None,
        });
        let cases = [case("code", "tools/tasks/cli.ts".into()), case("keyword", "FR-X-1".into()), case("cross", many(&["FR-X-1", "FR-X-9"]))];
        let err = check_anchors("f", None, &cases, &graph, &files_of(&graph)).unwrap_err().to_string();
        assert!(err.contains("\"tools/tasks/cli.ts\""), "the first stale anchor: {err}");
        assert!(err.contains("\"FR-X-9\""), "and the one after it: {err}");
        assert!(!err.contains("\"FR-X-1\""), "an anchor the graph holds is not named: {err}");
    }

    /// The pinned corpus moved `staleClaims` from `tools/tasks/src/cli.ts` to
    /// `packages/task-sync/src/cli.ts`; the question did not change, so neither does the case. A
    /// name defined twice is ambiguous and a name defined nowhere is gone: both stay refused.
    #[test]
    fn a_code_anchor_follows_its_symbol_to_the_one_file_that_defines_it_now() {
        use crate::model::Node;
        let sym = |id: &str, label: &str, file: &str| Node {
            id: id.into(), kind: NodeKind::Symbol, label: label.into(), body: String::new(), file: file.into(), line: 1, end: 0,
            files: Default::default(), community: None,
        };
        let mut graph = Graph::default();
        for (id, label, file) in [
            ("sym:packages/task-sync/src/cli.ts::staleClaims", "staleClaims", "packages/task-sync/src/cli.ts"),
            ("sym:a/x.ts::twice", "twice", "a/x.ts"),
            ("sym:b/x.ts::twice", "twice", "b/x.ts"),
        ] { graph.nodes.insert(id.into(), sym(id, label, file)); }
        let code = |q: &str, file: &str| Case { kind: "code".into(), q: q.into(), expect: file.into() };
        let mut cases = vec![code("staleClaims", "tools/tasks/src/cli.ts"), code("twice", "c/x.ts"), code("gone", "d/x.ts")];
        let files = files_of(&graph);
        let moves = relocate_moved(&mut cases, &graph, &files);
        assert_eq!(moves, ["\"staleClaims\" moved: tools/tasks/src/cli.ts -> packages/task-sync/src/cli.ts"]);
        assert_eq!(cases[0].expect.key(), "packages/task-sync/src/cli.ts");
        assert_eq!((cases[1].expect.key(), cases[2].expect.key()), ("c/x.ts".into(), "d/x.ts".into()));
        assert!(check_anchors("f", None, &cases[..1], &graph, &files).is_ok());
        assert!(check_anchors("f", None, &cases[1..], &graph, &files).is_err());
    }

    /// The stale-anchor refusal is the first thing a reader meets when they bench a corpus that
    /// has moved on from the pin. Without the pin in it, the obvious repair is to edit the case
    /// to the new path -- which is how the built-in suite stopped running on the tree its floors
    /// were counted on.
    #[test]
    fn a_stale_anchor_names_the_pin_the_cases_were_recorded_against() {
        use crate::model::{Node, NodeKind};
        let mut graph = Graph::default();
        graph.nodes.insert("FR-X-1".into(), Node {
            id: "FR-X-1".into(), kind: NodeKind::Requirement, label: String::new(), body: String::new(), file: "docs/x.md".into(), line: 1, end: 0,
            files: Default::default(), community: None,
        });
        let cases = [case("code", "packages/task-sync/src/cli.ts".into())];
        let err = check_anchors("f", Some("beauty-crm 502e8a6d"), &cases, &graph, &files_of(&graph)).unwrap_err().to_string();
        assert!(err.contains("beauty-crm 502e8a6d"), "{err}");
        assert!(err.contains("missing-anchors.py"), "and how to list the rest: {err}");
    }

    #[test]
    fn only_a_checkout_readably_off_the_pin_is_ungated() {
        let pin = "beauty-crm 502e8a6d";
        assert_eq!(off_pin(pin, Some("502e8a6d1234abcd\n")), None, "the pin itself is graded");
        assert_eq!(off_pin(pin, Some("a3bf96ff9999\n")), Some("a3bf96ff".into()));
        assert_eq!(off_pin(pin, None), None, "a tree whose commit cannot be read is graded");
        assert_eq!(off_pin(pin, Some("")), None);
        assert_eq!(off_pin("beauty-crm", Some("a3bf96ff")), None, "a pin without a commit ungates nothing");
    }

    /// `<corpus> <commit>`, one line: the error message quotes it whole, and the script beside it
    /// is what checks the cases against a checkout of that commit.
    #[test]
    fn the_pin_names_a_corpus_and_a_commit() {
        let pin: Vec<&str> = BUILT_IN_PIN.split_whitespace().collect();
        assert_eq!(pin.len(), 2, "{BUILT_IN_PIN:?}");
        assert!(pin[1].len() >= 7 && pin[1].chars().all(|c| c.is_ascii_hexdigit()), "{BUILT_IN_PIN:?}");
    }

    #[test]
    fn cases_file_parsing_skips_blank_lines() {
        let text = "\n{\"kind\":\"keyword\",\"q\":\"a\",\"expect\":\"X\"}\n\n   \n{\"kind\":\"code\",\"q\":\"b\",\"expect\":\"Y\"}\n";
        let cases = parse_cases(text).unwrap();
        assert_eq!(cases.iter().map(|c| c.kind.as_str()).collect::<Vec<_>>(), ["keyword", "code"]);
    }

    #[test]
    fn cases_file_of_only_blank_lines_yields_no_cases() {
        assert!(parse_cases("\n\n   \n").unwrap().is_empty());
    }

    #[test]
    fn cases_file_parsing_rejects_a_malformed_line() {
        let good = "{\"kind\":\"keyword\",\"q\":\"a\",\"expect\":\"X\"}\n";
        assert!(parse_cases(&format!("{good}not json\n")).is_err(), "invalid JSON");
        assert!(parse_cases(&format!("{good}{{\"kind\":\"keyword\",\"q\":\"a\"}}\n")).is_err(), "missing required field");
    }
}
