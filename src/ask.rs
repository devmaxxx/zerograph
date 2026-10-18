//! The work behind `zerograph ask`, shaped so it can be done more than once against one open
//! store: `Context::open` reads the graph and the ids, and `Context::answer`
//! turns a `Request` into the text the command prints. A one-shot `ask` opens a context, answers
//! once and leaves; a resident process opens one and answers many times, over the same code.

use crate::{config, index, model, query, refresh, rerank, store, walk};
use anyhow::Context as _;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};

/// `ZEROGRAPH_TIMING=1` prints where an `ask` spends its time, one line per stage on stderr.
pub(crate) struct Timing { on: bool, start: std::time::Instant, last: Cell<std::time::Instant> }

pub(crate) fn timing_on() -> bool { crate::env_os("TIMING").is_some() }

impl Timing {
    pub(crate) fn new() -> Timing {
        let now = std::time::Instant::now();
        Timing { on: timing_on(), start: now, last: Cell::new(now) }
    }

    pub(crate) fn stage(&self, what: &str) {
        if !self.on { return; }
        let now = std::time::Instant::now();
        eprintln!("timing: {:>7.1} ms  (+{:>6.1} ms)  {what}", (now - self.start).as_secs_f64() * 1e3, (now - self.last.get()).as_secs_f64() * 1e3);
        self.last.set(now);
    }
}

/// One line per writer phase under `ZEROGRAPH_TIMING`, in milliseconds since the first mark. A
/// writer has no `Timing` to carry: `apply_diff` is shared by every refresh path, and threading
/// one through each of them would be a signature change for a diagnostic. The clock starts at the
/// first mark of the process, so the offsets read as phases only in a one-shot `build` or
/// `update`; under `serve` they keep counting from the first refresh.
pub(crate) fn mark(what: &str) {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    if !timing_on() { return; }
    let start = *START.get_or_init(std::time::Instant::now);
    eprintln!("timing: {:>9.1} ms  {what}", start.elapsed().as_secs_f64() * 1e3);
}

/// What a model open yields, named because it also crosses a thread boundary.
pub(crate) type Opened = Result<Option<index::embed::Embedder>, String>;

/// The embedder, or the line that says why there is none — returned rather than printed, so a
/// process whose stderr is a socket reply can hand that line to the client that asked. An arm
/// with no dense side is `Ok(None)`: nothing was wanted and nothing is missing.
pub(crate) fn embedder_or_notice(no_dense: bool, model: &str, threads: usize, weights: index::embed::Weights) -> Opened {
    if no_dense { return Ok(None); }
    index::embed::Embedder::open(model, threads, weights)
        .map(Some)
        .map_err(|err| format!("dense: model unavailable, continuing lexical-only ({err:#})"))
}

/// The same open for the writers, whose stderr is the reader's terminal.
pub(crate) fn open_embedder(no_dense: bool, model: &str, threads: usize, weights: index::embed::Weights) -> Option<index::embed::Embedder> {
    embedder_or_notice(no_dense, model, threads, weights).unwrap_or_else(|notice| { eprintln!("{notice}"); None })
}

/// A notice for the client of this answer, said once: the model is resolved when it is warmed and
/// again when the open finds no warm handle, and both would tell the same story.
fn say_once(notices: &RefCell<Vec<String>>, line: String) {
    let mut n = notices.borrow_mut();
    if !n.contains(&line) { n.push(line); }
}

/// The model opens on a thread this spawns, while `Context::answer` builds its BM25 indexes on
/// the caller's own thread — `Lexical::build` runs between this spawn and the `query::ask` call,
/// so the two overlap there rather than by anything ordered inside `ask` itself. Ids are read
/// before the vectors that name the model, so they are not part of it.
/// A question that exact ids or symbols answer whole never opens the model, as before; with
/// `--no-dense` or no vectors nothing starts.
pub(crate) fn warm_model(dense: bool, whole: bool, model: &str, threads: usize, weights: index::embed::Weights) -> Option<std::thread::JoinHandle<Opened>> {
    if !dense || whole { return None; }
    let model = model.to_string();
    Some(std::thread::spawn(move || embedder_or_notice(false, &model, threads, weights)))
}

/// What a reader's look at the tree left it holding: the graph to answer from, what bringing it
/// in line cost, and, when it was not brought in line, what it is behind by.
pub(crate) struct Read {
    pub graph: model::Graph,
    pub refreshed: Option<crate::UpdateReport>,
    /// The files the graph is behind the tree by, with the line that says so, when the refresh
    /// did not fit the budget or another writer held the lock.
    pub behind: Option<(Vec<String>, String)>,
    pub writer: Writer,
    /// A line the reader is owed about the store itself rather than about the tree: there is
    /// none, or a newer release wrote it.
    pub note: Option<String>,
}

/// Whether this reader may write the store, decided once at the walk and held to by the dense
/// sync after it.
pub(crate) enum Writer {
    /// It took the lock and refreshed (or had nothing to refresh), and keeps the lock until it
    /// exits, so its own vector sync cannot race a detached one.
    Held(#[allow(dead_code)] store::WriterLock),
    /// The store could not be locked at all — a read-only checkout — so it writes as it always
    /// did, and a write that fails says so.
    Unlocked,
    /// It answers from the store as it stands: another writer held the lock, or the refresh was
    /// left to a detached one. Either way the vectors are that writer's too.
    Barred,
    /// It did not look: `--stale`, or a resident context whose watcher owns the walk. Its dense
    /// sync takes the lock for itself, per answer.
    PerAnswer,
}

#[cfg(test)]
thread_local! {
    /// Runs once between the reader's load and its lock: the window another release's save can
    /// land in, which no test could otherwise hit.
    pub(crate) static BEFORE_LOCK: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
}

/// The stored graph brought in line with the working tree when that fits `budget`, plus what it
/// cost. `ask` runs this before answering so an edit never has to be followed by an `update`; the
/// extractors are built only when there is something to re-read. A refresh over budget is left
/// to a detached `update`, and the stored graph answers.
pub(crate) fn graph_for_ask(repo: &Path, cfg: &config::Config, store: &store::Store, stale: bool, timing: &Timing, budget: &refresh::Budget) -> anyhow::Result<Read> {
    let graph_seen = store.stamp("graph.json");
    let (mut graph, mut manifest, mut source) = store.load_traced()?;
    timing.stage("graph loaded");
    // Before the lock: taking it writes into the store, and this build may write nothing into a
    // newer one.
    if manifest.newer_grammar() {
        return Ok(Read { graph, refreshed: None, behind: None, writer: Writer::Barred, note: Some(manifest.newer_store_line()) });
    }
    // Taking the lock makes the directory, and a read from a directory nobody built would leave
    // a store behind in it; building one is `build`'s to do.
    if !store.stands() {
        let note = format!("index: none at {} — `zerograph build` builds it", repo.display());
        return Ok(Read { graph, refreshed: None, behind: None, writer: Writer::Barred, note: Some(note) });
    }
    if stale { return Ok(Read { graph, refreshed: None, behind: None, writer: Writer::PerAnswer, note: None }); }
    #[cfg(test)]
    BEFORE_LOCK.with(|h| h.borrow_mut().take().map(|f| f()));
    let writer = match store.try_lock_writer() {
        Ok(Some(lock)) => Writer::Held(lock),
        Ok(None) => Writer::Barred,
        Err(_) if store.has_link() => Writer::Barred,
        Err(_) => Writer::Unlocked,
    };
    // A newer release's save can land between the load above and the lock; read past it, this
    // build's grammar would be stamped over that store by the refresh or the stamps below. The
    // manifest is read again rather than its stamp compared: a save of the same length inside the
    // file system's mtime granularity leaves the stamp as it was. An unlocked writer writes too.
    // The graph's stamp is checked as well: `import-legacy` saves a new graph under the manifest
    // it loaded, and a refresh from the graph in hand would save over its edges.
    if matches!(writer, Writer::Held(_) | Writer::Unlocked) {
        let fresh = store.load_manifest()?;
        if fresh.newer_grammar() {
            return Ok(Read { graph, refreshed: None, behind: None, writer: Writer::Barred, note: Some(fresh.newer_store_line()) });
        }
        if fresh != manifest || store.stamp("graph.json") != graph_seen {
            (graph, manifest, source) = store.load_traced()?;
            if manifest.newer_grammar() {
                return Ok(Read { graph, refreshed: None, behind: None, writer: Writer::Barred, note: Some(manifest.newer_store_line()) });
            }
        }
    }
    let linked = matches!(writer, Writer::Barred) && store.has_link();
    let entries = walk::walk(repo, cfg, &manifest)?;
    let diff = manifest.diff(&entries);
    timing.stage("tree walked");
    let busy = matches!(writer, Writer::Barred);
    let unlocked = matches!(writer, Writer::Unlocked);
    // A store an older grammar wrote holds less than the tree says it does, and no hash reports
    // it, so an unchanged tree is not on its own a reason to answer from what is there.
    if diff.changed.is_empty() && diff.removed.is_empty() && !manifest.stale_grammar() {
        // A reader that found the lock held writes nothing at all: the stamps it would record
        // are older than the manifest the holder is about to save.
        if busy { return Ok(Read { graph, refreshed: None, behind: None, writer, note: None }); }
        crate::record_stamps(store, &manifest, &entries)?;
        // A store another release or a bare `graph.json` left without a mirror pays the JSON
        // parse once; a refresh below writes the mirror on its own.
        if let store::Source::Json(read) = source {
            store.write_mirror("graph.json", read, &graph)?;
            timing.stage("mirror written");
        }
        return Ok(Read { graph, refreshed: None, behind: None, writer, note: None });
    }
    let work = match manifest.stale_grammar() { true => entries.len(), false => diff.changed.len() + diff.removed.len() };
    if busy || !budget.fits(work, refresh::FILE_COST) {
        // Released before the spawn, though the refresh would wait for it: there is nothing left
        // for this process to write, and no reason to hold the next writer up until it exits.
        drop(writer);
        let what = refresh::files_line(work);
        // A store that cannot be locked cannot be written by a detached `update` either.
        let line = match (unlocked, linked) {
            (true, _) => format!("{what}; the store cannot be locked for a background refresh — run `zerograph update`"),
            (_, true) => format!("{what}; the store holds a symlink no writer follows — run `zerograph update` for which"),
            _ => refresh::left_behind(repo, budget.no_dense, &what, busy),
        };
        // A grammar-stale store re-reads every file, so every file is what it is behind by.
        let files = match manifest.stale_grammar() {
            true => { let mut all: Vec<String> = entries.iter().map(|e| e.rel.clone()).collect(); all.sort(); all }
            false => refresh::files_of(&diff),
        };
        return Ok(Read { graph, refreshed: None, behind: Some((files, line)), writer: Writer::Barred, note: None });
    }
    let r = crate::apply_diff(repo, store, &mut graph, &entries, &diff, &manifest, &crate::extractors(repo, cfg)?)?;
    timing.stage("refreshed");
    Ok(Read { graph, refreshed: Some(r), behind: None, writer, note: None })
}

/// The most cache shards a reader's lookup reads, each whole: near a megabyte apiece at the default
/// cap, so an edit's few rows are read and a pull's worth is left to the refresh it starts.
const READER_CACHE_SHARDS: usize = 32;

/// A reader's sync in small steps that grow, so the first one measures the machine's rate before
/// much of the budget is spent: 16 rows, then 32, up to 256.
const READER_CHUNK: index::dense::ChunkBudget = index::dense::ChunkBudget { chars: 40_000, max_rows: 256, ramp: 16 };

/// One question and the flags it is asked under — everything `Cmd::Ask` carries that is not the
/// store itself, so a request can cross a socket without the context moving with it.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct Request { pub words: Vec<String>, pub json: bool, pub seeds: usize, pub bodies: bool, pub rerank: bool, pub rerank_local: bool, pub depth: usize, pub stale: bool, pub no_dense: bool }

/// An open store ready to answer. Everything that costs more than a question to build — the
/// graph, the vectors, the embedding model, the cross-encoder — is held here and
/// kept between answers.
pub struct Context {
    cfg: config::Config,
    store: store::Store,
    graph: model::Graph,
    /// The BM25 indexes over `graph`, built by the first answer that fuses and kept until it
    /// moves. Built there rather than in `open` so that on the first fused
    /// question the build still overlaps the model open, as it did when `query::ask` built it.
    lexical: RefCell<Option<index::lexical::Lexical>>,
    no_dense: bool,
    dense_idx: RefCell<Option<index::dense::DenseIndex>>,
    warm: RefCell<Option<std::thread::JoinHandle<Opened>>>,
    embedder: RefCell<Option<Option<index::embed::Embedder>>>,
    cross: RefCell<Option<index::cross::CrossEncoder>>,
    /// The `graph.json` stamp `graph` was read at or written to — what the vectors' own claim is
    /// held against before a fused answer. `None` for a store the refresh could not write, whose
    /// vectors are answered from as they stand.
    graph_at: Option<walk::Stamp>,
    notices: RefCell<Vec<String>>,
    /// The rerank commands started since the last `take_reranked`: what a paid allowance counts,
    /// which neither a refused command nor an exact-id answer that never asks the model adds to.
    reranked: Cell<u32>,
    timing: Timing,
    repo: PathBuf,
    writer: Writer,
    /// When the answer in progress has to be printed by. Set at `open` for a one-shot, whose
    /// refresh counts against it, and again by a resident process for each request.
    budget: Cell<refresh::Budget>,
    /// What the answer in progress was given without; `files` is the open's, `vectors` the
    /// answer's own.
    stale: RefCell<Option<refresh::Stale>>,
}

impl Context {
    /// The store read the way `ask` reads it: refreshed against the tree unless `stale`, the
    /// stored graph with a warning when the store cannot be written.
    pub fn open(repo: &Path, cfg: &config::Config, stale: bool, no_dense: bool) -> anyhow::Result<Context> {
        let timing = Timing::new();
        let budget = refresh::Budget::seconds(cfg.reader_budget, no_dense);
        let store = store::Store::new(repo);
        let mut notices = Vec::new();
        // Stamped before the read, never after; a refresh rewrites the file and reports the stamp
        // of its own bytes, since a `stat` after it may already see another writer's graph.
        let read_at = store.stamp("graph.json");
        let (graph, refreshed, graph_at, behind, writer) = match graph_for_ask(repo, cfg, &store, stale, &timing, &budget) {
            Ok(Read { graph, refreshed, behind, writer, note }) => {
                notices.extend(note);
                let at = refreshed.as_ref().map_or(read_at, |r| r.graph_at);
                (graph, refreshed, at, behind, writer)
            }
            // A store that cannot be written (read-only checkout, a walk that failed) still
            // holds an answer: say once that it may be behind, then give the stored one.
            Err(err) => { notices.push(format!("refresh: skipped ({err:#})")); (store.load()?.0, None, None, None, Writer::Unlocked) }
        };
        if let Some(r) = &refreshed { notices.push(format!("refresh: {} changed, {} removed", r.changed, r.removed)); }
        let stale_report = behind.map(|(files, line)| { notices.push(line); refresh::Stale { files, vectors: 0 } });
        Ok(Context {
            cfg: cfg.clone(), store, graph, no_dense,
            lexical: RefCell::new(None),
            dense_idx: RefCell::new(None),
            warm: RefCell::new(None),
            embedder: RefCell::new(None),
            cross: RefCell::new(None),
            graph_at,
            notices: RefCell::new(notices),
            reranked: Cell::new(0),
            timing,
            repo: repo.to_path_buf(),
            writer,
            budget: Cell::new(budget),
            stale: RefCell::new(stale_report),
        })
    }

    /// Starts the budget over for a request a resident process is about to answer, whose clock
    /// starts when the question arrives and not when the process did. Whatever the last answer
    /// was given without is that answer's, and goes with it.
    pub(crate) fn begin(&self) {
        self.budget.set(refresh::Budget::seconds(self.cfg.reader_budget, self.no_dense));
        *self.stale.borrow_mut() = None;
    }

    /// The arm this context was opened in. A resident process hands it to every client in the
    /// handshake: the `||` below cannot widen a lexical context back to a fused one, so a fused
    /// question has to be refused before it is asked rather than answered lexically.
    pub fn no_dense(&self) -> bool { self.no_dense }

    /// Whether this context is holding model weights right now — the embedder, the local
    /// reranker, or a warm open still running. A slot that holds a *failed* open holds no
    /// memory, so it does not count.
    pub fn model_open(&self) -> bool {
        self.embedder.borrow().as_ref().is_some_and(Option::is_some)
            || self.cross.borrow().is_some()
            || self.warm.borrow().is_some()
    }

    /// Forgets the model weights and keeps everything else: the graph, the lexical indexes and
    /// the vectors stay resident, so a lexical answer is still milliseconds
    /// and a fused one pays the ~220 ms open again. The slots are the same ones the answer path
    /// fills lazily, so nothing has to be told the model went. Returns whether anything was held.
    ///
    /// A warm open still in flight is joined rather than abandoned: dropping the handle alone
    /// would leave the thread to finish and hold the weights nobody can reach any more.
    pub fn drop_model(&self) -> bool {
        let held = self.model_open();
        if let Some(handle) = self.warm.borrow_mut().take() { let _ = handle.join(); }
        *self.embedder.borrow_mut() = None;
        *self.cross.borrow_mut() = None;
        held
    }

    /// The text `ask` prints for this request — `render`'s output, byte for byte.
    pub fn answer(&mut self, req: &Request) -> anyhow::Result<String> {
        // A context opened without the dense arm has no vectors and no model to grow one from,
        // so it narrows a fused request and never the other way; `serve` refuses that pairing.
        let no_dense = self.no_dense || req.no_dense;
        let opts = query::Options { seeds: req.seeds, bodies: req.bodies, dense: !no_dense && index::dense::DenseIndex::present(&self.store), json: req.json, depth: req.depth };
        let Context { cfg, store, graph, lexical, dense_idx, warm, embedder, cross, graph_at, notices, reranked, timing, repo, writer, budget, stale, .. } = &*self;
        // Opening the ONNX model costs ~220 ms and 1.3 GB, the vectors 50 MB; an exact id or
        // symbol match never asks for either, so on that path both still open lazily, on the
        // first fused query that never comes. A fused question starts both below, once the
        // last fallible step is behind them.
        let dense_fn = |q: &str, k: usize| -> Vec<String> {
            // The vectors come first because they name the model: reading `vectors.json`
            // again through `recorded_model` would parse 3 MB a second time for what the
            // loaded index already holds.
            let mut idx = dense_idx.borrow_mut();
            let idx = idx.get_or_insert_with(|| {
                let i = index::dense::DenseIndex::load(store).unwrap_or_else(|err| { notices.borrow_mut().push(format!("dense: index unreadable, continuing lexical-only ({err:#})")); Default::default() });
                timing.stage("vectors loaded"); i
            });
            // A model named in `zerograph.toml` after the store was embedded takes effect at the
            // next `update`; a reader that only asks would otherwise keep the old one forever.
            // This answer still comes from the stored vectors — the rewrite is a whole re-embed.
            if !req.stale && index::embed::switch_owed(config::names_embed_model(repo), idx.model_of_rows().as_deref(), &cfg.embed_model) {
                let what = format!("dense: {} names {}, the vectors are {}'s", config::project_file_name(repo), cfg.embed_model, idx.model_of_rows().unwrap_or_default());
                let busy = match writer { Writer::Barred => true, Writer::PerAnswer => matches!(store.try_lock_writer(), Ok(None)), _ => false };
                say_once(notices, refresh::left_behind_as(repo, no_dense, &what, busy, "switching"));
            }
            let mut slot = embedder.borrow_mut();
            let e = slot.get_or_insert_with(|| {
                // Collected, never printed: on the warm thread this line used to race the main
                // thread's own stderr, and over a socket it would land on the server's terminal
                // instead of reaching the client whose answer went lexical-only because of it.
                let opened = match warm.borrow_mut().take() {
                    Some(handle) => handle.join().unwrap_or_else(|_| Err("dense: model thread panicked, continuing lexical-only".to_string())),
                    None => {
                        index::embed::reader_model(idx.model_of_rows().as_deref(), &cfg.embed_model).and_then(|model|
                            embedder_or_notice(no_dense, &model, index::embed::threads(cfg.resources), index::embed::Weights::Packed))
                    }
                };
                timing.stage("model opened");
                opened.unwrap_or_else(|notice| { notices.borrow_mut().push(notice); None })
            });
            let qvec = e.as_mut().and_then(|e| e.query(q).ok());
            // Decided before a single row is written: the resync below embeds and saves, so a
            // store whose rows are not this model's width has to be refused here — after it,
            // the notice would be an epitaph for the index the resync had already replaced.
            if let (Some(v), Some(emb)) = (&qvec, e.as_ref()) {
                if idx.dim > 0 && v.len() != idx.dim {
                    notices.borrow_mut().push(format!("dense: {}; continuing lexical-only", index::embed::width_mismatch(idx.dim, emb.name(), v.len())));
                    return Vec::new();
                }
            }
            // `--stale` asks for the store as it is and pays for no walk; embedding rows and
            // saving them is the most expensive thing this code does, and a one-shot `--stale`
            // never reaches it. What is owed is the vectors' own claim against the graph in hand,
            // not something this process remembers: a refresh here, an `update --no-dense`, and an
            // `ask` an exact id answered after refreshing all leave the same store behind.
            if !req.stale && graph_at.is_some_and(|at| idx.behind(at)) {
                if let Some(emb) = e.as_mut() {
                    // `Config::load` refused a glob that does not parse, so only a config built
                    // in code reaches the default.
                    let skip = index::dense::EmbedSkip::new(&cfg.embed_skip).unwrap_or_default();
                    // Rows owed and not embedded here: said once, and carried to `--json`. The
                    // refresh that will embed them is started unless one is running already.
                    let owe = |left: usize, start: bool| {
                        let what = refresh::vectors_line(left);
                        let line = match (start, stale.borrow().is_some()) {
                            (true, _) => refresh::left_behind(repo, no_dense, &what, false),
                            // The graph's refresh was left to a detached `update` a moment ago,
                            // and that one embeds as well.
                            (false, true) => what,
                            (false, false) => refresh::left_behind(repo, no_dense, &what, true),
                        };
                        notices.borrow_mut().push(line);
                        stale.borrow_mut().get_or_insert_with(Default::default).vectors = left;
                    };
                    // A resident context takes the lock for this sync alone, and finding it held
                    // is a refresh another process is running — the same as a one-shot's.
                    let per_answer = matches!(writer, Writer::PerAnswer).then(|| store.try_lock_writer());
                    if matches!(writer, Writer::Barred) || matches!(per_answer, Some(Ok(None))) || (matches!(per_answer, Some(Err(_))) && store.has_link()) {
                        let left = idx.owed(graph, &skip);
                        if left > 0 { owe(left, false); }
                    } else {
                        // A reader appends to the store's own rows and never re-embeds them into
                        // another model's index: it claims the index for the model it opened, at
                        // the width this very query just measured.
                        let width = qvec.as_ref().map_or(idx.dim, |v| v.len());
                        idx.written_by(emb.name(), width);
                        idx.use_cache(index::cache::VectorCache::machine(cfg.vector_cache_max), emb.snapshot());
                        idx.bound_cache(READER_CACHE_SHARDS);
                        let budget = budget.get();
                        // Taken from the cache first, so the budget weighs only the rows that must
                        // be embedded. The lookup is bounded, since it runs before the budget is
                        // weighed: rows past the bound are owed, and the refresh takes them. The
                        // deadline was set when the request began, so the lookup's time is already
                        // spent from it; `started` times the embeds alone, whose rate is projected.
                        let lent = idx.take_cached(graph, &skip);
                        let owed = idx.owed(graph, &skip);
                        let started = std::time::Instant::now();
                        // Weighed at an idle machine's rate first, so a pull's worth of rows is
                        // not started at all; past that, each chunk's own rate decides. A lookup
                        // cut short may have left cached rows among the owed ones, and embedding
                        // them here would also write them to the cache a second time.
                        let synced = match !idx.lookup_cut() && budget.fits(owed, refresh::ROW_COST) {
                            true => idx.sync_chunked(graph, &skip, &mut |texts| emb.embed(texts), READER_CHUNK, &mut |_, p| {
                                match budget.overrun(p.done, p.total, started.elapsed()) {
                                    true => Err(refresh::OutOfBudget(p.total - p.done).into()),
                                    false => Ok(()),
                                }
                            }),
                            false => Err(refresh::OutOfBudget(owed).into()),
                        };
                        match synced {
                            Ok(n) => {
                                idx.synced_against(*graph_at);
                                // Saved with nothing embedded as well: the claim is what spares the
                                // next answer this pass.
                                if let Err(err) = idx.save(store) { notices.borrow_mut().push(format!("refresh: vectors not saved ({err:#})")); }
                                if n > 0 { notices.borrow_mut().push(format!("refresh: {n} vectors embedded")); }
                            }
                            Err(err) => match err.downcast_ref::<refresh::OutOfBudget>() {
                                Some(&refresh::OutOfBudget(left)) => {
                                    // What was embedded or cached before the stop is kept: the
                                    // refresh that finishes matches those rows by hash and embeds
                                    // the rest.
                                    if left < owed || lent > 0 {
                                        idx.checkpointed();
                                        if let Err(err) = idx.save(store) { notices.borrow_mut().push(format!("refresh: vectors not saved ({err:#})")); }
                                    }
                                    owe(left, true);
                                }
                                None => notices.borrow_mut().push(format!("refresh: vectors unchanged ({err:#})")),
                            },
                        }
                    }
                    drop(per_answer);
                    timing.stage("vectors synced");
                }
            }
            let out = match qvec { Some(v) => idx.search(&v, k), None => Vec::new() };
            timing.stage("query embedded and searched");
            out
        };
        // Collected like the local reranker's failure beside it, so `--rerank` and
        // `--rerank-local` tell a client the same thing when their model will not answer.
        let rerank_fn = |q: &str, c: &[(String, String)]| {
            if let Some(why) = cfg.refusal("rerank_command") {
                notices.borrow_mut().push(format!("rerank: {why}; answering from the fused order"));
                return Vec::new();
            }
            reranked.set(reranked.get() + 1);
            let (picked, notice) = rerank::run_or_notice(&cfg.rerank_command, q, c);
            if let Some(n) = notice { notices.borrow_mut().push(n); }
            picked
        };
        if req.rerank_local && cross.borrow().is_none() {
            let dir = if cfg.reranker_dir.is_empty() { index::cross::default_dir()? } else { PathBuf::from(&cfg.reranker_dir) };
            *cross.borrow_mut() = Some(index::cross::CrossEncoder::open(&dir, index::embed::threads(cfg.resources)).context("--rerank-local")?);
        }
        // Below the last `?`: an error returned between the spawn and the join drops the
        // handle and leaves a thread mid-open of a 448 MB session. Nothing below this point
        // can fail, and the only work above it the open might have overlapped is
        // `--rerank-local`'s own session.
        //
        // The predicate `ask` fuses on, asked once here so it can gate both the model warm-up
        // below and the BM25 build further down: an exact id or symbol answers the question
        // whole, and `query::ask` never reads a lexical list or a dense row on that path
        // (`!whole_question`), so neither is worth paying for. `exact_seeds` is pure and
        // `query::ask` re-asks it itself, so asking it here too costs one graph scan and
        // decides nothing differently.
        let (_, whole) = query::exact_seeds(graph, &req.words);
        if opts.dense && !whole {
            let mut slot = dense_idx.borrow_mut();
            let idx = slot.get_or_insert_with(|| {
                let i = index::dense::DenseIndex::load(store).unwrap_or_else(|err| { notices.borrow_mut().push(format!("dense: index unreadable, continuing lexical-only ({err:#})")); Default::default() });
                timing.stage("vectors loaded"); i
            });
            // The rows name the model, so nothing can start before they are read. A context
            // that has already opened the model keeps it rather than starting a second one.
            let mut handle = warm.borrow_mut();
            if handle.is_none() && embedder.borrow().is_none() {
                // A model turned away is not warmed; the open below says why, once.
                if let Ok(model) = index::embed::reader_model(idx.model_of_rows().as_deref(), &cfg.embed_model) {
                    *handle = warm_model(true, whole, &model, index::embed::threads(cfg.resources), index::embed::Weights::Packed);
                }
            }
        }
        let local_fn = |q: &str, c: &[(String, String)]| -> Vec<String> {
            let mut m = cross.borrow_mut();
            let Some(m) = m.as_mut() else { return Vec::new() };
            let texts: Vec<String> = c.iter().map(|(_, t)| t.clone()).collect();
            match m.score(q, &texts) {
                Ok(s) => index::cross::pick(&s, c, index::cross::PICK),
                // Like a failing rerank command: say so and answer from the fused order.
                Err(e) => { notices.borrow_mut().push(format!("rerank-local: {e:#}; answering from the fused order")); Vec::new() }
            }
        };
        let rerank: Option<query::Rerank> = if req.rerank_local { Some(&local_fn) } else if req.rerank { Some(&rerank_fn) } else { None };
        let mut guard = lexical.borrow_mut();
        // A question `whole` answers never reaches `lexical_lists` either (same gate as the
        // dense warm-up above), so building anything here would be pure loss — and caching an
        // empty `Lexical` would leave the next question that does fuse answering from an index
        // over nothing. `Lexical::empty()` is built fresh each time and dropped with this call.
        let mut empty = None;
        let lex: &index::lexical::Lexical = if whole {
            empty.get_or_insert_with(index::lexical::Lexical::empty)
        } else {
            guard.get_or_insert_with(|| { let l = index::lexical::Lexical::build(graph); timing.stage("lexical built"); l })
        };
        let answer = query::ask(graph, lex, Some(&dense_fn), rerank, &req.words, &opts);
        timing.stage("answered");
        let text = query::render(&answer, graph, &opts);
        Ok(match (req.json, stale.borrow().as_ref()) {
            (true, Some(s)) => refresh::with_stale(text, s),
            _ => text,
        })
    }

    /// The watcher's graph taken over after a poll moved it, which is how a resident context
    /// reaches the state a one-shot `ask` would have loaded from disk — without reading ten
    /// megabytes back to learn what the poll already holds. `refreshed` is the poll's own
    /// report when it applied a change and `None` when it only read a store someone else wrote.
    pub(crate) fn adopt(&mut self, w: &crate::Watcher, refreshed: Option<crate::UpdateReport>) -> anyhow::Result<()> {
        self.graph = w.graph.clone();
        self.graph_at = w.graph_at;
        // A newer release may have written the store since this context opened; from then on its
        // per-answer vector sync would write into a store this build may not write.
        if w.manifest.newer_grammar() { self.writer = Writer::Barred; }
        // An adopted graph is a new population, so the indexes built over the old one cannot
        // outlive this call.
        *self.lexical.borrow_mut() = None;
        // The vectors another process rewrote, dropped so the next fused answer loads them —
        // reading what is on disk is what a one-shot does, and it is not the same as embedding
        // the rows again here, which would write a store nobody asked this process to write.
        let moved = { let idx = self.dense_idx.borrow(); idx.as_ref().is_some_and(|i| i.read_at() != self.store.stamp("vectors.f32")) };
        if moved { *self.dense_idx.borrow_mut() = None; }
        // Whether the adopted graph is owed rows is the vectors' claim against `graph_at`, asked
        // by the next fused answer. A refresh the watcher applied and a store another writer moved
        // without embedding — lefthook's `update --no-dense` — owe them alike; a writer that did
        // embed claimed its own graph, and owes none.
        if let Some(r) = refreshed {
            self.notices.borrow_mut().push(format!("refresh: {} changed, {} removed", r.changed, r.removed));
        }
        Ok(())
    }

    /// A line for this answer's client, said beside the ones the answer itself collects.
    pub(crate) fn note(&self, line: String) { self.notices.borrow_mut().push(line); }

    /// Notices `ask` used to print on stderr for this answer (refresh lines, dense fallbacks), drained.
    pub fn notices(&mut self) -> Vec<String> { std::mem::take(&mut self.notices.borrow_mut()) }

    pub fn take_reranked(&self) -> u32 { self.reranked.replace(0) }

    pub(crate) fn timing(&self) -> &Timing { &self.timing }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_is_not_warmed_for_an_exact_answer_or_a_lexical_arm() {
        assert!(warm_model(false, false, "any", 4, index::embed::Weights::Packed).is_none());
        assert!(warm_model(true, true, "any", 4, index::embed::Weights::Packed).is_none());
    }

    // The `**ID · MUST · label**` form is what declares a requirement; an id in a heading
    // is only a reference to one, and leaves the graph with nothing to answer from.
    fn repo_with_two_docs() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("docs/pay.md"), "# Оплата\n\n**FR-PAY-1 · MUST · Штраф за отмену**\n\nШтраф списывается сам.\n").unwrap();
        std::fs::write(dir.path().join("docs/cal.md"), "# Календарь\n\n**FR-CAL-1 · MUST · Перенос визита**\n\nПеренос не считается отменой.\n").unwrap();
        std::fs::write(dir.path().join("zerograph.toml"), "id_families = [\"FR-PAY\", \"FR-CAL\"]\n").unwrap();
        dir
    }

    /// What `serve --idle-model` calls between questions. A context that never opened a model
    /// says it held nothing, and the slots it empties are the ones the answer path fills lazily —
    /// so the next fused question opens a model exactly as the first one did.
    #[test]
    fn dropping_the_model_empties_the_slots_the_answer_path_fills() {
        let dir = repo_with_two_docs();
        let cfg = crate::config::Config::load(dir.path()).unwrap();
        crate::run_update(dir.path(), &cfg, true).unwrap();
        let mut ctx = Context::open(dir.path(), &cfg, true, true).unwrap();
        assert!(!ctx.model_open());
        assert!(!ctx.drop_model(), "nothing was held");
        let req = Request { words: vec!["штраф".into()], json: false, seeds: 5, bodies: false, rerank: false, rerank_local: false, depth: crate::rerank::DEPTH, stale: true, no_dense: true };
        let before = ctx.answer(&req).unwrap();
        ctx.drop_model();
        assert_eq!(ctx.answer(&req).unwrap(), before, "the answer a dropped model leaves behind is the same answer");
    }

    /// A paid allowance is charged by the model calls an answer starts: an exact id is answered
    /// without one, a fused question starts one even when the command then fails.
    #[test]
    fn only_a_started_rerank_command_is_counted() {
        let dir = repo_with_two_docs();
        let mut cfg = crate::config::Config::load(dir.path()).unwrap();
        cfg.rerank_command = "exit 1".into();
        crate::run_update(dir.path(), &cfg, true).unwrap();
        let mut ctx = Context::open(dir.path(), &cfg, true, true).unwrap();
        let ask = |words: &[&str]| Request { words: words.iter().map(|w| w.to_string()).collect(), json: false, seeds: 5, bodies: false, rerank: true, rerank_local: false, depth: crate::rerank::DEPTH, stale: true, no_dense: true };
        ctx.answer(&ask(&["FR-PAY-1"])).unwrap();
        assert_eq!(ctx.take_reranked(), 0, "an exact id never reached the model");
        ctx.answer(&ask(&["штраф", "отмена"])).unwrap();
        assert_eq!(ctx.take_reranked(), 1, "{:?}", ctx.notices());
        assert_eq!(ctx.take_reranked(), 0, "taking the count resets it");
    }

    // A lock that is a link refuses every writer; a reader that took the refusal for a read-only
    // checkout would refresh and save with no lock at all.
    #[cfg(unix)]
    #[test]
    fn a_reader_whose_lock_is_a_link_writes_nothing() {
        let dir = repo_with_two_docs();
        let cfg = crate::config::Config::load(dir.path()).unwrap();
        crate::run_update(dir.path(), &cfg, true).unwrap();
        let store = store::Store::new(dir.path());
        let target = dir.path().join("elsewhere");
        std::fs::write(&target, "").unwrap();
        let lock = dir.path().join(".zerograph/writer.lock");
        let _ = std::fs::remove_file(&lock);
        std::os::unix::fs::symlink(&target, &lock).unwrap();
        std::fs::write(dir.path().join("docs/pay.md"), "# Оплата\n\n**FR-PAY-2 · MUST · Новое**\n\nтекст\n").unwrap();
        let manifest = std::fs::read(dir.path().join(".zerograph/manifest.json")).unwrap();
        let read = graph_for_ask(dir.path(), &cfg, &store, false, &Timing::new(), &refresh::Budget::seconds(cfg.reader_budget, true)).unwrap();
        assert!(matches!(read.writer, Writer::Barred));
        assert!(read.refreshed.is_none());
        assert!(read.behind.is_some_and(|(_, line)| line.contains("symlink")), "no detached refresh is promised");
        assert_eq!(std::fs::read(dir.path().join(".zerograph/manifest.json")).unwrap(), manifest, "nothing was saved");
    }

    #[test]
    fn a_context_answers_the_same_text_twice_and_for_an_exact_id() {
        let dir = repo_with_two_docs();
        let cfg = crate::config::Config::load(dir.path()).unwrap();
        crate::run_update(dir.path(), &cfg, true).unwrap();
        let mut ctx = Context::open(dir.path(), &cfg, true, true).unwrap();
        let req = Request { words: vec!["штраф".into()], json: false, seeds: 5, bodies: false, rerank: false, rerank_local: false, depth: crate::rerank::DEPTH, stale: true, no_dense: true };
        let first = ctx.answer(&req).unwrap();
        let second = ctx.answer(&req).unwrap();
        assert_eq!(first, second);
        assert!(first.contains("FR-PAY-1"), "{first}");
        let exact = ctx.answer(&Request { words: vec!["FR-CAL-1".into()], ..req }).unwrap();
        assert!(exact.lines().next().unwrap().contains("  FR-CAL-1  "), "{exact}");
    }

    /// The dense arm as a machine without the model has it, prepared once for the whole binary:
    /// a cache in hf-hub's layout whose model file is 64 MB of nothing. The fetch is answered
    /// from disk — at that size the weights are taken to be inside the file, so nothing is looked
    /// up over the network — and the session then refuses to open it. Without this the two tests
    /// below would download 470 MB, or open the model already on the machine and embed with it.
    /// It is laid out under the name `resolve` returns rather than under the default one, so
    /// `ZEROGRAPH_EMBED_MODEL` in the environment moves the fixture with it instead of missing
    /// it and sending `repo.get` to the network.
    fn an_unopenable_model() {
        static CACHE: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        CACHE.get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            let model = index::embed::writer_model(index::embed::DEFAULT_MODEL);
            let (hub, variant) = index::embed::hub_and_variant(&model);
            let root = dir.path().join(format!("models--{hub}").replace('/', "--"));
            let rev = index::embed::revision(&model).unwrap_or("abc");
            let snap = root.join("snapshots").join(rev);
            std::fs::create_dir_all(root.join("refs")).unwrap();
            std::fs::create_dir_all(snap.join("onnx")).unwrap();
            std::fs::write(root.join("refs/main"), rev).unwrap();
            std::fs::File::create(snap.join(index::embed::variant_file("onnx/model.onnx", variant))).unwrap().set_len(64 << 20).unwrap();
            std::fs::write(snap.join("tokenizer.json"), "{}").unwrap();
            std::fs::write(snap.join("config.json"), r#"{"pad_token_id": 1}"#).unwrap();
            std::fs::write(snap.join("tokenizer_config.json"), r#"{"pad_token": "<pad>"}"#).unwrap();
            // Process-wide, which is why it is set once behind the lock: no other test in this
            // binary opens an embedder — the rest are `--no-dense`, and the fetch tests are
            // handed their cache by argument. Set through the crate rather than through the
            // environment, which no test may write while the rest of the binary reads it.
            index::embed::set_cache_dir(dir.path().to_path_buf());
            dir
        });
    }

    /// A store with vectors, so `answer` takes the dense arm at all, claimed for the graph beside
    /// them as the sync that wrote them would have. The rows are invented: nothing here searches
    /// them, because the model will not open.
    ///
    /// The rows name their model rather than leaving it blank, and they name the same constant
    /// `an_unopenable_model` lays its cache out under. A blank one resolves through
    /// `UNNAMED_MODEL` instead, which is deliberately *not* the default — so a blank field would
    /// send this test looking for a model the fixture never cached, and out to the network to
    /// find it.
    fn vectors_beside_the_graph(repo: &Path) -> (PathBuf, PathBuf) {
        let (json, raw) = (repo.join(".zerograph/vectors.json"), repo.join(".zerograph/vectors.f32"));
        let model = index::embed::DEFAULT_MODEL;
        let graph = serde_json::to_string(&store::Store::new(repo).stamp("graph.json")).unwrap();
        std::fs::write(&json, format!(r#"{{"ids":["FR-PAY-1"],"hashes":["h"],"kinds":[false],"dim":4,"model":"{model}","graph":{graph}}}"#)).unwrap();
        std::fs::write(&raw, [0u8; 16]).unwrap();
        (json, raw)
    }

    fn fused(stale: bool) -> Request {
        Request { words: vec!["штраф".into()], json: false, seeds: 5, bodies: false, rerank: false, rerank_local: false, depth: crate::rerank::DEPTH, stale, no_dense: false }
    }

    /// What the next fused answer decides its catch-up on: the vectors on disk against the graph
    /// the context holds.
    fn owed(ctx: &Context) -> bool {
        let idx = index::dense::DenseIndex::load(&ctx.store).unwrap();
        ctx.graph_at.is_some_and(|at| idx.behind(at))
    }

    const NEW_DOC: &str = "**FR-PAY-2 · MUST · Возврат аванса**\n\nАванс возвращается при отмене салоном.\n";

    #[test]
    fn vectors_claimed_for_the_graph_in_hand_owe_no_rows() {
        let dir = repo_with_two_docs();
        let cfg = crate::config::Config::load(dir.path()).unwrap();
        crate::run_update(dir.path(), &cfg, true).unwrap();
        vectors_beside_the_graph(dir.path());
        let ctx = Context::open(dir.path(), &cfg, false, false).unwrap();
        assert!(!owed(&ctx), "an unchanged store pays for no pass on every question");
    }

    #[test]
    fn a_refresh_at_open_owes_the_rows_it_moved() {
        let dir = repo_with_two_docs();
        let cfg = crate::config::Config::load(dir.path()).unwrap();
        crate::run_update(dir.path(), &cfg, true).unwrap();
        vectors_beside_the_graph(dir.path());
        std::fs::write(dir.path().join("docs/new.md"), NEW_DOC).unwrap();
        let ctx = Context::open(dir.path(), &cfg, false, false).unwrap();
        assert!(owed(&ctx), "FR-PAY-2 has no row");
    }

    // What lefthook's `update --no-dense` leaves behind: the graph and the manifest moved with the
    // tree, the vectors did not. Nothing is left for the next `ask` to refresh, and the row the
    // update never embedded is owed all the same.
    #[test]
    fn a_row_an_update_without_the_model_left_unembedded_is_owed_by_the_next_fused_answer() {
        let dir = repo_with_two_docs();
        let cfg = crate::config::Config::load(dir.path()).unwrap();
        crate::run_update(dir.path(), &cfg, true).unwrap();
        vectors_beside_the_graph(dir.path());
        std::fs::write(dir.path().join("docs/new.md"), NEW_DOC).unwrap();
        crate::run_update(dir.path(), &cfg, false).unwrap();
        let ctx = Context::open(dir.path(), &cfg, false, false).unwrap();
        assert!(owed(&ctx), "FR-PAY-2 has no row");
    }

    #[test]
    fn a_resident_context_owes_the_rows_of_a_store_an_update_without_the_model_moved() {
        let dir = repo_with_two_docs();
        let cfg = crate::config::Config::load(dir.path()).unwrap();
        crate::run_update(dir.path(), &cfg, true).unwrap();
        vectors_beside_the_graph(dir.path());
        let mut ctx = Context::open(dir.path(), &cfg, true, false).unwrap();
        let mut w = crate::Watcher::open(dir.path(), &cfg).unwrap();
        std::fs::write(dir.path().join("docs/new.md"), NEW_DOC).unwrap();
        crate::run_update(dir.path(), &cfg, false).unwrap();
        assert!(matches!(w.poll(1).unwrap(), crate::Polled::Quiet) && w.reloaded, "the poll read the other writer's store back");
        ctx.adopt(&w, None).unwrap();
        assert!(owed(&ctx), "FR-PAY-2 has no row");
    }

    #[test]
    fn a_resident_context_owes_the_rows_its_own_watcher_refreshed() {
        let dir = repo_with_two_docs();
        let cfg = crate::config::Config::load(dir.path()).unwrap();
        crate::run_update(dir.path(), &cfg, true).unwrap();
        vectors_beside_the_graph(dir.path());
        let mut ctx = Context::open(dir.path(), &cfg, true, false).unwrap();
        let mut w = crate::Watcher::open(dir.path(), &cfg).unwrap();
        std::fs::write(dir.path().join("docs/new.md"), NEW_DOC).unwrap();
        let crate::Polled::Refreshed(r) = w.poll(1).unwrap() else { panic!("new.md is a refresh") };
        ctx.adopt(&w, Some(r)).unwrap();
        assert!(owed(&ctx), "FR-PAY-2 has no row");
    }

    #[test]
    fn vectors_another_process_rewrote_are_dropped_rather_than_kept() {
        let dir = repo_with_two_docs();
        let cfg = crate::config::Config::load(dir.path()).unwrap();
        crate::run_update(dir.path(), &cfg, true).unwrap();
        let (_, raw) = vectors_beside_the_graph(dir.path());
        an_unopenable_model();
        let mut ctx = Context::open(dir.path(), &cfg, true, false).unwrap();
        ctx.answer(&fused(true)).unwrap();
        assert!(ctx.dense_idx.borrow().is_some(), "the fused answer loaded the vectors");
        let w = crate::Watcher::open(dir.path(), &cfg).unwrap();
        ctx.adopt(&w, None).unwrap();
        assert!(ctx.dense_idx.borrow().is_some(), "vectors nobody rewrote are the ones already in hand");
        // What an `embed` in another process leaves behind: the same file, different rows.
        std::fs::write(&raw, [0u8; 32]).unwrap();
        ctx.adopt(&w, None).unwrap();
        assert!(ctx.dense_idx.borrow().is_none(), "the next fused answer reads them off disk instead");
    }

    // `serve`'s poll only ever calls `adopt` when it already decided something moved (a refresh
    // or a reload), so `adopt` itself invalidates unconditionally rather than diffing — this
    // pins that an answer built after one `adopt` is not the pair a stale answer would have
    // fused from.
    #[test]
    fn lexical_indexes_built_by_an_answer_are_dropped_once_the_context_adopts_a_watcher() {
        let dir = repo_with_two_docs();
        let cfg = crate::config::Config::load(dir.path()).unwrap();
        crate::run_update(dir.path(), &cfg, true).unwrap();
        let mut ctx = Context::open(dir.path(), &cfg, true, true).unwrap();
        ctx.answer(&fused(true)).unwrap();
        assert!(ctx.lexical.borrow().is_some(), "the first fused answer built and kept the indexes");
        let w = crate::Watcher::open(dir.path(), &cfg).unwrap();
        ctx.adopt(&w, None).unwrap();
        assert!(ctx.lexical.borrow().is_none(), "an adopted graph invalidates the held indexes");
        ctx.answer(&fused(true)).unwrap();
        assert!(ctx.lexical.borrow().is_some(), "the next fused answer rebuilds them");
    }
}
