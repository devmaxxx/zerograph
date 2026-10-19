use crate::model::{Graph, NodeKind};
use crate::store::Store;
use crate::walk::Stamp;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Node ids best first, each with the cosine of its best row.
pub type Scored = Vec<(String, f32)>;

/// How far a chunked sync has got, for the caller that checkpoints and says so.
pub struct Progress { pub done: usize, pub total: usize }

/// Holes tolerated per live row before `sync` compacts. Compaction costs the whole-file rewrite
/// the append exists to avoid, so it is worth a quarter of the file being dead weight.
const HOLE_SHARE: usize = 4;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct DenseIndex {
    /// One entry per row of `vectors.f32`, in the file's own order: a row keeps its offset for
    /// as long as it lives, which is what lets a sync append instead of rewriting 50 MB. A dead
    /// row holds an empty id until the next compaction closes the gap.
    pub ids: Vec<String>,
    pub hashes: Vec<String>,
    /// True for a generated-question row, which only a store an older binary enriched holds:
    /// `search` skips them and the next sync retires them.
    #[serde(default)] pub kinds: Vec<bool>,
    pub dim: usize,
    /// Hub id of the model every row was embedded with. Empty in a store written before the
    /// field existed — which only the small model ever wrote.
    #[serde(default)] pub model: String,
    /// The `graph.json` stamp of the graph the rows were last brought in line with, claimed by a
    /// sync that finished and never by a checkpoint. A writer that moves the graph and embeds
    /// nothing — `update --no-dense`, a model that would not open, an `ask` an exact id answered
    /// — leaves it behind, which is how the next reader holding a model learns it owes rows. A
    /// store written before the field claims no graph and is caught up once.
    #[serde(default)] graph: Option<Stamp>,
    /// The dead rows, ascending. Left out of the file when there are none, so a store this
    /// binary wrote and never punched a hole in still reads in one that predates the field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")] free: Vec<usize>,
    #[serde(skip)] pub vectors: Vec<f32>,
    /// The live rows, ascending — what `search` scans, so a dead row is not even a branch on
    /// the hot path.
    #[serde(skip)] live: Vec<usize>,
    /// Leading rows of `vectors.f32` that already hold what memory holds, with the stamp of the
    /// file they were counted in: an append may only extend a file no one else has rewritten.
    #[serde(skip)] persisted: usize,
    #[serde(skip)] stamp: Option<Stamp>,
    /// The machine's vector cache, which a sync asks before it embeds and writes every embedded
    /// row through to. `None` — a test, or a cache turned off or refused — embeds everything.
    #[serde(skip)] cache: Option<super::cache::VectorCache>,
    /// The commit the model's files were fetched at, which the cache keys rows by beside the id.
    #[serde(skip)] revision: String,
    /// How many of the cache's shards one lookup may read, when a reader set a bound.
    #[serde(skip)] cache_shards: Option<usize>,
    /// The width `written_by` claimed. A cold index has no width until its first chunk is
    /// embedded, and a cached row has to be looked up at one before that.
    #[serde(skip)] claimed_dim: usize,
    /// Set by `take_cached`, so the sync that follows it does not ask the cache a second time
    /// for rows it was just asked for and did not hold.
    #[serde(skip)] looked_up: bool,
}

/// Every text embedded for a node, e5-prefixed. A file has no passage row — its head comment is
/// for the prompt, not the index. A text file is its body in rows of at most `TEXT_ROW_CHARS`: the
/// embedder reads 256 tokens of a row and drops the rest, so one row would leave everything past a
/// file's first screen unreachable.
fn rows(n: &crate::model::Node) -> Vec<String> {
    match n.kind {
        NodeKind::File => Vec::new(),
        NodeKind::Text => text_rows(&n.label, &n.body),
        _ => vec![format!("passage: {}\n{}", n.label, n.indexed_body())],
    }
}

/// The files `embed_skip` names: their nodes stay in the graph and the lexical index, and have no
/// rows here. Matched on the node's own `file`, the path a `skip` glob would match it by, so a
/// requirement a skipped file only mentions keeps the rows its declaring document gives it.
#[derive(Default)]
pub struct EmbedSkip(Option<globset::GlobSet>);

impl EmbedSkip {
    pub fn new(globs: &[String]) -> Result<EmbedSkip> {
        if globs.is_empty() { return Ok(EmbedSkip(None)); }
        crate::walk::globs(globs).map(|g| EmbedSkip(Some(g))).context("embed_skip")
    }

    /// The rows `n` is owed under this skip: none for a node of a skipped file.
    fn rows(&self, n: &crate::model::Node) -> Vec<String> {
        match &self.0 {
            Some(g) if g.is_match(&n.file) => Vec::new(),
            _ => rows(n),
        }
    }
}

/// Under 256 tokens for the small model in Cyrillic, its densest script (about 2.5 chars a token).
const TEXT_ROW_CHARS: usize = 600;

fn text_rows(label: &str, body: &str) -> Vec<String> {
    // The label rides in every row and the embedder counts it too, so the body gets what is left;
    // half the row at least, so a deep path cannot shrink a chunk to nothing.
    let room = TEXT_ROW_CHARS.saturating_sub(label.chars().count()).max(TEXT_ROW_CHARS / 2);
    let mut chunks = Vec::new();
    let (mut cur, mut len) = (String::new(), 0usize);
    for line in body.split_inclusive('\n') {
        for piece in pieces(line, room) {
            let n = piece.chars().count();
            if len + n > room && !cur.is_empty() {
                chunks.push(std::mem::take(&mut cur));
                len = 0;
            }
            cur.push_str(piece);
            len += n;
        }
    }
    chunks.push(cur);
    if chunks.iter().all(|c| c.trim().is_empty()) {
        return Vec::new();
    }
    chunks.into_iter().map(|c| format!("passage: {label}\n{c}")).collect()
}

/// `s` in runs of at most `n` chars, cut on char boundaries.
fn pieces(s: &str, n: usize) -> impl Iterator<Item = &str> {
    let mut rest = s;
    std::iter::from_fn(move || {
        if rest.is_empty() { return None; }
        let cut = rest.char_indices().nth(n).map_or(rest.len(), |(i, _)| i);
        let (head, tail) = rest.split_at(cut);
        rest = tail;
        Some(head)
    })
}

/// Which model a store's rows belong to, given the name it records and whether it holds rows at
/// all. Rows with no name are the small model's — the only model that ever wrote an unnamed store
/// — so a reader opens that whatever the configuration says, and no reader can move a store to
/// another model. `None` is for a store with no rows for a name to be wrong about, where the
/// configuration is free to choose.
fn model_of(named: &str, has_rows: bool) -> Option<String> {
    if !named.is_empty() { return Some(printable(named)); }
    has_rows.then(|| crate::index::embed::UNNAMED_MODEL.to_string())
}

/// The name a store records is a clone's text, and every report, status line and hook brief that
/// prints it would otherwise carry a newline or an escape sequence straight to a terminal or an
/// agent's context. A real model name has no control character, so the escaped form is never one
/// this machine has trusted.
fn printable(name: &str) -> String {
    name.chars().fold(String::with_capacity(name.len()), |mut out, c| {
        match c.is_control() { true => out.extend(c.escape_default()), false => out.push(c) }
        out
    })
}

fn drop_taken<T>(v: &mut Vec<T>, taken: &[bool]) {
    let mut i = 0;
    v.retain(|_| { i += 1; !taken[i - 1] });
}

fn normalise(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 { for x in v { *x /= norm; } }
}
fn le_bytes(v: &[f32]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(v.len() * 4);
    for x in v { raw.extend_from_slice(&x.to_le_bytes()); }
    raw
}

/// How much work one checkpoint covers. Characters rather than rows because a row is not a unit of
/// work — 1,024 short questions and 1,024 long passages are the same count and a minute apart — and
/// characters rather than tokens because the tokenizer lives behind the embedder and this plan is
/// made before a single forward: the two are the same shape, and the bar this exists to meet is a
/// progress line every 60 seconds, not a token count. `ramp` divides the budget for the first chunk
/// and halves its way out of the division after each one, so the first line does not wait for the
/// run's fixed start-up — 2.24 GB of weights paged in as the first forwards touch them — *and* a
/// full chunk on top of it.
#[derive(Clone, Copy, Debug)]
pub struct ChunkBudget { pub chars: usize, pub max_rows: usize, pub ramp: usize }

impl ChunkBudget {
    /// A fixed number of rows and no ramp: what a test or a one-chunk sync asks for.
    #[cfg(test)]
    pub const fn rows(n: usize) -> Self { ChunkBudget { chars: usize::MAX, max_rows: n, ramp: 1 } }
}

/// Where each checkpoint falls, as end offsets into `texts`. A single row over budget on its own
/// still gets a chunk; the alternative is a chunk that never ends.
pub fn chunk_ends(texts: &[String], b: ChunkBudget) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut divisor = b.ramp.max(1);
    let (mut chars, mut rows) = (0usize, 0usize);
    for (i, t) in texts.iter().enumerate() {
        chars = chars.saturating_add(t.chars().count());
        rows += 1;
        if chars >= b.chars / divisor || rows >= (b.max_rows / divisor).max(1) {
            ends.push(i + 1);
            divisor = (divisor / 2).max(1);
            chars = 0;
            rows = 0;
        }
    }
    if !texts.is_empty() && ends.last() != Some(&texts.len()) { ends.push(texts.len()); }
    ends
}

impl DenseIndex {
    /// Whether an index is on disk, without reading it: an exact-id or `--no-dense` answer
    /// never needs the vectors, and loading 50 MB of them cost every such `ask` 40 ms.
    pub fn present(store: &Store) -> bool {
        store.has("vectors.json") && store.has("vectors.f32")
    }

    /// The model a store's rows belong to, read from `vectors.json` without loading the rows —
    /// what a reader opens. The configured model takes effect at the next `build`, `update`
    /// or `embed`, which rewrites the index whole.
    pub fn recorded_model(store: &Store) -> Result<Option<String>> {
        #[derive(serde::Deserialize)]
        struct Written { #[serde(default)] model: String }
        let Some(meta) = store.read_bytes("vectors.json")? else { return Ok(None) };
        let w: Written = serde_json::from_slice(&meta).context("vectors.json")?;
        // `has` stats the file rather than reading it: 50 MB of rows must not be loaded to learn
        // whether there are any.
        Ok(model_of(&w.model, store.has("vectors.f32")))
    }

    /// The stamp of the `vectors.f32` these rows were read from, for a reader that holds an
    /// index across requests: a file that no longer matches was rewritten by someone else, and
    /// what is in hand is no longer what an `ask` starting now would load.
    pub fn read_at(&self) -> Option<Stamp> { self.stamp }

    /// Whether rows are owed to the graph stamped `graph`: the last finished sync was against
    /// another one, or claimed none.
    pub fn behind(&self, graph: Stamp) -> bool { self.graph != Some(graph) }

    /// Claims the graph the sync that just returned brought the rows in line with. `sync` drops
    /// the claim as it starts, so a caller that makes none leaves the index owing rows: the next
    /// reader pays a pass, and no row goes missing.
    pub fn synced_against(&mut self, graph: Option<Stamp>) { self.graph = graph; }

    /// `recorded_model`'s answer for an index already in hand, so a reader that has loaded the
    /// vectors does not parse a 3 MB `vectors.json` again to learn the same thing.
    pub fn model_of_rows(&self) -> Option<String> {
        model_of(&self.model, !self.ids.is_empty())
    }

    /// Claims the index for `model`, whose vectors are `dim` wide, before a sync. Rows another
    /// model wrote cannot be appended to or compared against, and neither can rows of another
    /// width: the name alone would miss a store the `ZEROGRAPH_EMBED_MODEL` recipe left holding
    /// wide rows under no name, where a claim by the small model's name matches, `sync` then
    /// matches every row by hash and embeds nothing, and the store is recorded as the small
    /// model's over rows it never wrote — a state no later `embed` could reach. Either mismatch
    /// drops the rows and the file is rewritten from the new ones alone. A hub id names one model
    /// in any case, so a claim that differs only in case keeps the rows and records its spelling.
    pub fn written_by(&mut self, model: &str, dim: usize) {
        let held = if self.model.is_empty() { crate::index::embed::UNNAMED_MODEL } else { self.model.as_str() };
        if !self.ids.is_empty() && (!held.eq_ignore_ascii_case(model) || (self.dim > 0 && self.dim != dim)) {
            self.ids.clear(); self.hashes.clear(); self.kinds.clear(); self.vectors.clear();
            self.free.clear(); self.live.clear();
            self.persisted = 0; self.dim = 0;
        }
        self.model = model.to_string();
        self.claimed_dim = dim;
    }

    /// Attaches the vector cache a sync asks and writes through to, for the model files fetched at
    /// `revision`.
    pub fn use_cache(&mut self, cache: Option<super::cache::VectorCache>, revision: &str) {
        self.cache = cache;
        self.revision = revision.to_string();
    }

    /// Caps a cache lookup at `shards` shards; the rows in the shards past it are not looked up.
    pub fn bound_cache(&mut self, shards: usize) { self.cache_shards = Some(shards); }

    pub fn load(store: &Store) -> Result<DenseIndex> {
        let Some(meta) = store.read_bytes("vectors.json")? else { return Ok(DenseIndex::default()) };
        let mut idx: DenseIndex = serde_json::from_slice(&meta).context("vectors.json")?;
        // Stamped before the read, never after: a rewrite racing this read then leaves a stamp
        // the next save cannot match, and it rewrites the file whole instead of appending to a
        // prefix that is no longer ours.
        idx.stamp = store.stamp("vectors.f32");
        let raw = store.read_bytes("vectors.f32")?.unwrap_or_default();
        let want = idx.ids.len() * idx.dim;
        if raw.len() / 4 < want {
            // A torn pair of files is treated as no index at all; the next sync rebuilds it.
            // The recorded name survives the empty index, though: dropped, the store would read
            // as unnamed and a refreshing `ask` would rebuild it under the configured model
            // rather than the one that wrote it.
            return Ok(DenseIndex { model: idx.model, ..Default::default() });
        }
        // Anything past the last row the metadata names is what a crash between an append and
        // the metadata rename left: unreferenced, and overwritten by the next append.
        #[allow(clippy::chunks_exact_to_as_chunks)]
        { idx.vectors = raw[..want * 4].chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(); }
        idx.persisted = idx.ids.len();
        idx.reindex();
        Ok(idx)
    }

    /// The per-row arrays lined up with the ids and the live scan order rebuilt. The resize is
    /// for stores older than the `kinds` field: without it the first appended row would land at
    /// index 0 of a short array and answer for someone else's row.
    fn reindex(&mut self) {
        self.hashes.resize(self.ids.len(), String::new());
        self.kinds.resize(self.ids.len(), false);
        let mut dead = vec![false; self.ids.len()];
        for &i in &self.free { if let Some(d) = dead.get_mut(i) { *d = true; } }
        self.free = dead.iter().enumerate().filter(|(_, d)| **d).map(|(i, _)| i).collect();
        self.live = dead.iter().enumerate().filter(|(_, d)| !**d).map(|(i, _)| i).collect();
    }

    /// The new rows appended and the metadata renamed over the old, in that order: a crash
    /// between the two leaves rows nothing points at, never metadata pointing at rows that are
    /// not there. The whole file is rewritten instead when what is in hand is no longer an
    /// extension of what is on disk — after a compaction, or when another process rewrote it.
    pub fn save(&mut self, store: &Store) -> Result<()> {
        let kept = self.persisted * self.dim;
        let extends = self.persisted > 0 && kept <= self.vectors.len()
            && self.stamp.is_some() && self.stamp == store.stamp("vectors.f32");
        if extends {
            store.append_after("vectors.f32", kept as u64 * 4, &le_bytes(&self.vectors[kept..]))?;
        } else {
            store.write_atomic("vectors.f32", &le_bytes(&self.vectors))?;
        }
        self.persisted = self.ids.len();
        // What was taken from the cache is saved now; a later sync asks it again.
        self.looked_up = false;
        self.stamp = store.stamp("vectors.f32");
        store.write_atomic("vectors.json", &serde_json::to_vec(self)?)
    }

    /// How many rows a `sync` against this graph would embed, without embedding any: what a
    /// reader weighs against its budget before it opens the work, and reports when it does not.
    pub fn owed(&self, graph: &Graph, skip: &EmbedSkip) -> usize {
        if self.dim == 0 { return graph.nodes.values().map(|n| skip.rows(n).len()).sum(); }
        let held: std::collections::HashSet<(&str, &str)> =
            self.live.iter().map(|&i| (self.ids[i].as_str(), self.hashes[i].as_str())).collect();
        graph.nodes.values()
            .map(|n| skip.rows(n).iter().filter(|t| !held.contains(&(n.id.as_str(), blake3::hash(t.as_bytes()).to_hex().as_str()))).count())
            .sum()
    }

    /// The rows the vector cache holds for this graph, taken in before anything counts what is
    /// owed: a cached row costs a read, not an embed, and counted as owed it would send a reader
    /// over its budget to a detached refresh, or put a meter's total above the work. The rows
    /// join the index as live rows, which the sync after this matches by hash and keeps. Returns
    /// how many were taken.
    pub fn take_cached(&mut self, graph: &Graph, skip: &EmbedSkip) -> usize {
        let held: std::collections::HashSet<(String, String)> = match self.dim {
            0 => Default::default(),
            _ => self.live.iter().map(|&i| (self.ids[i].clone(), self.hashes[i].clone())).collect(),
        };
        // Once per pair: the sync after this keeps one row per (id, hash) and would leave a
        // second copy as a hole.
        let mut todo = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for n in graph.nodes.values() {
            for text in skip.rows(n) {
                let pair = (n.id.clone(), blake3::hash(text.as_bytes()).to_hex().to_string());
                if !held.contains(&pair) && seen.insert(pair.clone()) { todo.push(pair); }
            }
        }
        let hashes: Vec<String> = todo.iter().map(|(_, h)| h.clone()).collect();
        let lent = self.cached(&hashes);
        self.looked_up = true;
        if lent.is_empty() { return 0; }
        if self.dim == 0 { self.ids.clear(); self.hashes.clear(); self.kinds.clear(); self.vectors.clear(); self.free.clear(); self.persisted = 0; }
        self.dim = self.claimed_dim;
        let mut taken = 0;
        for (id, hash) in todo {
            let Some(v) = lent.get(hash.as_str()) else { continue };
            self.ids.push(id);
            self.hashes.push(hash);
            self.kinds.push(false);
            self.vectors.extend_from_slice(v);
            taken += 1;
        }
        self.reindex();
        taken
    }

    /// The index in hand made searchable again after a sync its callback stopped: the rows that
    /// sync appended join the scan beside the ones they will replace, which is the state a
    /// checkpoint saved to disk loads into. Without it a resident reader would scan only the old
    /// rows, and its next sync, matching against those alone, would embed the new ones twice.
    pub fn checkpointed(&mut self) { self.looked_up = false; self.reindex(); }

    #[cfg(test)]
    #[allow(clippy::type_complexity)]
    pub fn sync(&mut self, graph: &Graph, skip: &EmbedSkip, embed: &mut dyn FnMut(&[String]) -> Result<Vec<Vec<f32>>>) -> Result<usize> {
        self.sync_chunked(graph, skip, embed, ChunkBudget::rows(usize::MAX), &mut |_, _| Ok(()))
    }

    /// `sync`, embedding a `budget`'s worth of rows at a time and calling `after_chunk` after each with the
    /// index in a state worth saving: the rows already embedded are appended, and every row the
    /// old index held is still alive and still at its offset. So a checkpoint an interrupted run
    /// leaves behind is a consistent store — the next sync matches the saved rows by hash,
    /// embeds only what is missing, and retires whatever the edit orphaned, at a cost of one
    /// duplicate row per edited node until then. The holes, the reindex and the compaction wait
    /// for the end for that reason: they are what makes the old rows unreachable, and a run that
    /// stops halfway must not have done half of it. A node `skip` covers is owed no rows, so the
    /// rows it held die with the rest of what the graph no longer asks for, and every other row
    /// is matched by hash and kept.
    #[allow(clippy::type_complexity)]
    pub fn sync_chunked(
        &mut self,
        graph: &Graph,
        skip: &EmbedSkip,
        embed: &mut dyn FnMut(&[String]) -> Result<Vec<Vec<f32>>>,
        budget: ChunkBudget,
        after_chunk: &mut dyn FnMut(&mut DenseIndex, Progress) -> Result<()>,
    ) -> Result<usize> {
        // A checkpoint `after_chunk` saves on the way is a store still owed rows — after a model
        // change it holds a part of the very graph it last claimed — so if the run stops there it
        // must read as in line with no graph.
        self.graph = None;
        let mut alive = vec![false; self.ids.len()];
        let mut todo_ids = Vec::new();
        let mut todo_texts = Vec::new();
        let mut todo_hashes = Vec::new();
        let mut todo_kinds = Vec::new();
        let old: std::collections::HashMap<(&str, &str), usize> =
            self.live.iter().map(|&i| ((self.ids[i].as_str(), self.hashes[i].as_str()), i)).collect();
        for n in graph.nodes.values() {
            for text in skip.rows(n) {
                let hash = blake3::hash(text.as_bytes()).to_hex().to_string();
                match old.get(&(n.id.as_str(), hash.as_str())).copied() {
                    Some(i) if self.dim > 0 => alive[i] = true,
                    _ => { todo_ids.push(n.id.clone()); todo_texts.push(text); todo_hashes.push(hash); todo_kinds.push(false); }
                }
            }
        }
        let lent = match std::mem::take(&mut self.looked_up) {
            true => Default::default(),
            false => self.cached(&todo_hashes),
        };
        if !lent.is_empty() {
            // A cold index learns its width here rather than from a first chunk; a warm one already
            // has this width, since `written_by` dropped rows of any other.
            self.dim = self.claimed_dim;
            let mut taken = Vec::with_capacity(todo_ids.len());
            for (i, hash) in todo_hashes.iter().enumerate() {
                let v = lent.get(hash.as_str());
                if let Some(v) = v {
                    self.ids.push(todo_ids[i].clone());
                    self.hashes.push(hash.clone());
                    self.kinds.push(false);
                    self.vectors.extend_from_slice(v);
                    alive.push(true);
                }
                taken.push(v.is_some());
            }
            drop_taken(&mut todo_ids, &taken);
            drop_taken(&mut todo_texts, &taken);
            drop_taken(&mut todo_hashes, &taken);
            drop_taken(&mut todo_kinds, &taken);
        }
        let embedded = todo_ids.len();
        let mut done = 0;
        let plan = chunk_ends(&todo_texts, budget);
        for &end in &plan {
            let mut vecs = embed(&todo_texts[done..end])?;
            for v in vecs.iter_mut() { normalise(v); }
            self.remember(&todo_hashes[done..end], &vecs);
            let dim = vecs.first().map(|v| v.len()).unwrap_or(self.dim);
            // A model of another width invalidates every stored offset, so the old rows cannot
            // be appended to — they go, and the file is rewritten from the new ones alone. Only
            // the first chunk can find that out: after it the index's width is this model's.
            if done == 0 && self.dim > 0 && dim != self.dim {
                self.ids.clear(); self.hashes.clear(); self.kinds.clear(); self.vectors.clear();
                alive.clear();
                self.persisted = 0;
            }
            self.dim = dim;
            for i in done..end {
                self.ids.push(todo_ids[i].clone());
                self.hashes.push(todo_hashes[i].clone());
                self.kinds.push(todo_kinds[i]);
                self.vectors.extend_from_slice(&vecs[i - done]);
                alive.push(true);
            }
            done = end;
            after_chunk(self, Progress { done, total: embedded })?;
        }
        self.free = alive.iter().enumerate().filter(|(_, a)| !**a).map(|(i, _)| i).collect();
        // A hole keeps its row's floats — that is what holds the offsets still — but not its
        // identity: nothing may match it again, and the metadata is rewritten on every save.
        for &i in &self.free { self.ids[i].clear(); self.hashes[i].clear(); self.kinds[i] = false; }
        self.reindex();
        if self.free.len() * HOLE_SHARE > self.live.len() { self.compact(); }
        Ok(embedded)
    }

    /// Vectors for as many of the `wanted` text hashes as the machine's cache holds. Only a sync
    /// that claimed a model and a width asks, and only for rows of that model at that width: a
    /// vector is a function of the model and the text alone, so such a row is the one this sync
    /// would embed — near-identical rather than bit-identical, since batch padding moves the last
    /// digits.
    fn cached(&self, wanted: &[String]) -> std::collections::HashMap<String, Vec<f32>> {
        let Some(cache) = self.cache.as_ref().filter(|_| self.claimed_dim > 0 && !wanted.is_empty()) else { return Default::default() };
        let asked: Vec<&str> = wanted.iter().map(String::as_str).collect();
        let found = match self.cache_shards {
            Some(shards) => cache.get_within(&self.model, &self.revision, self.claimed_dim, &asked, shards).0,
            None => cache.get(&self.model, &self.revision, self.claimed_dim, &asked),
        };
        if !found.is_empty() {
            let rows = wanted.iter().filter(|h| found.contains_key(h.as_str())).count();
            eprintln!("dense: {rows} rows from the vector cache");
        }
        found
    }

    /// Rows this process just embedded, written through to the cache — the one place anything
    /// enters it, so it holds nothing this machine did not embed itself. A chunk of a width other
    /// than the claimed one is the model contradicting its claim, and is kept out.
    fn remember(&self, hashes: &[String], vecs: &[Vec<f32>]) {
        let Some(cache) = self.cache.as_ref().filter(|_| self.claimed_dim > 0) else { return };
        let rows: Vec<(&str, &[f32])> = hashes.iter().zip(vecs)
            .filter(|(_, v)| v.len() == self.claimed_dim)
            .map(|(h, v)| (h.as_str(), v.as_slice())).collect();
        if let Err(err) = cache.put(&self.model, &self.revision, self.claimed_dim, &rows) {
            super::cache::warn_once("not written", &err);
        }
    }

    /// The live rows closed up. Every offset moves, so the next save rewrites both files whole,
    /// exactly as every save did before the rows became append-only.
    fn compact(&mut self) {
        let live = std::mem::take(&mut self.live);
        let mut vectors = Vec::with_capacity(live.len() * self.dim);
        let mut ids = Vec::with_capacity(live.len());
        let mut hashes = Vec::with_capacity(live.len());
        let mut kinds = Vec::with_capacity(live.len());
        for &i in &live {
            vectors.extend_from_slice(&self.vectors[i * self.dim..(i + 1) * self.dim]);
            ids.push(std::mem::take(&mut self.ids[i]));
            hashes.push(std::mem::take(&mut self.hashes[i]));
            kinds.push(self.kinds[i]);
        }
        self.ids = ids;
        self.hashes = hashes;
        self.kinds = kinds;
        self.vectors = vectors;
        self.free.clear();
        self.live = (0..self.ids.len()).collect();
        self.persisted = 0;
        if self.ids.is_empty() { self.dim = 0; }
    }

    /// The passage rows, a node once by its best row.
    pub fn search(&self, query: &[f32], k: usize) -> Vec<String> {
        self.search_scored(query, k).into_iter().map(|(id, _)| id).collect()
    }

    /// `search` with each node's best cosine.
    pub fn search_scored(&self, query: &[f32], k: usize) -> Scored {
        if self.dim == 0 || query.len() != self.dim { return Vec::new(); }
        let mut q = query.to_vec();
        normalise(&mut q);
        let mut rows: Vec<(f32, &str)> = self.live.iter().copied()
            .filter(|&i| !self.kinds[i])
            .map(|i| {
                let v = &self.vectors[i * self.dim..(i + 1) * self.dim];
                (v.iter().zip(&q).map(|(a, b)| a * b).sum::<f32>(), self.ids[i].as_str())
            }).collect();
        rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(b.1)));
        let mut seen = std::collections::HashSet::new();
        rows.iter().filter(|r| seen.insert(r.1)).take(k).map(|r| (r.1.to_string(), r.0)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Extraction, NodeKind};

    fn graph(body22: &str) -> Graph {
        let mut g = Graph::default();
        let mut e = Extraction::default();
        e.node(NodeKind::Requirement, "FR-PAY-22", "отмена", body22, "a.md", 1);
        e.node(NodeKind::Requirement, "FR-PAY-26", "штраф", "списание", "a.md", 9);
        e.node(NodeKind::File, "file:a.md", "a.md", "", "a.md", 1);
        g.apply(e);
        g
    }

    /// Ten nodes with distinct three-byte labels, `edited` of them carrying a body that differs
    /// from the default one. Wide enough that a handful of holes stays under the compaction
    /// share, which is what the append path needs to be observable.
    fn wide(edited: u32) -> Graph {
        let mut g = Graph::default();
        let mut e = Extraction::default();
        for i in 0..10u32 {
            let body = if i < edited { "изменённое" } else { "тело" };
            e.node(NodeKind::Requirement, &format!("FR-W-{i}"), &format!("n{i}x"), body, "w.md", i + 1);
        }
        g.apply(e);
        g
    }

    /// A stand-in embedder: a 3-d vector from the first three bytes after the e5 prefix, so
    /// tests are deterministic.
    fn fake(texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| {
            let b = t.split_once(": ").map(|x| x.1).unwrap_or(t).as_bytes();
            vec![b[0] as f32, b.get(1).copied().unwrap_or(0) as f32, b.get(2).copied().unwrap_or(0) as f32]
        }).collect())
    }

    /// The same stand-in two floats wider, for the claims that turn on the model's width rather
    /// than its name.
    fn fake_wide(texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(fake(texts)?.into_iter().map(|mut v| { v.extend_from_slice(&[0.0, 1.0]); v }).collect())
    }

    fn synced(g: &Graph) -> DenseIndex {
        let mut idx = DenseIndex::default();
        idx.sync(g, &EmbedSkip::default(), &mut fake).unwrap();
        idx
    }

    /// A row is not a unit of work, and the whole-store rebuild's cadence is the proof: a fixed
    /// 1,024 rows put the first progress line at 102.4 s against a 60 s bar and the last chunks
    /// 96.7 s apart, because the graph's iteration order puts the long passages last. The budget
    /// is characters, and the first chunks are fractions of it so the first line does not wait for
    /// the run's fixed start-up as well as for a full chunk.
    #[test]
    fn a_chunk_is_bounded_by_characters_and_by_rows_and_the_first_one_is_small() {
        let short: Vec<String> = (0..4096).map(|i| format!("row {i}")).collect();
        let long: Vec<String> = (0..4096).map(|i| format!("{} {i}", "слово ".repeat(250))).collect();
        let b = ChunkBudget { chars: 600_000, max_rows: 1024, ramp: 16 };
        let s = chunk_ends(&short, b);
        assert_eq!(s[0], 64, "the first chunk is the budget's sixteenth");
        assert_eq!(s[1] - s[0], 128, "and it doubles until the budget");
        assert!(s.windows(2).all(|w| w[1] - w[0] <= 1024), "no chunk is over the row cap");
        assert_eq!(*s.last().unwrap(), short.len(), "every row is in a chunk");
        let l = chunk_ends(&long, b);
        assert!(l[1] - l[0] < 200, "a chunk of long passages ends on characters: {}", l[1] - l[0]);
        assert_eq!(*l.last().unwrap(), long.len());
    }

    #[test]
    fn sync_chunked_appends_every_row_and_reports_progress_after_each_chunk() {
        let mut idx = DenseIndex::default();
        let mut seen = Vec::new();
        let n = idx.sync_chunked(&wide(0), &EmbedSkip::default(), &mut fake, ChunkBudget::rows(4),
            &mut |_, p| { seen.push((p.done, p.total)); Ok(()) }).unwrap();
        assert_eq!(n, 10);
        assert_eq!(seen, vec![(4, 10), (8, 10), (10, 10)]);
        let whole = synced(&wide(0));
        assert_eq!((idx.ids, idx.hashes, idx.vectors), (whole.ids, whole.hashes, whole.vectors));
    }

    #[test]
    fn a_checkpoint_saved_mid_sync_loads_and_the_next_sync_finishes_the_rest() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        let mut idx = DenseIndex::default();
        // A run killed after its first checkpoint: the rows it embedded are on disk.
        let err = idx.sync_chunked(&wide(0), &EmbedSkip::default(), &mut fake, ChunkBudget::rows(4), &mut |i, _| {
            i.save(&store)?;
            anyhow::bail!("interrupted")
        }).unwrap_err().to_string();
        assert!(err.contains("interrupted"), "{err}");
        let mut back = DenseIndex::load(&store).unwrap();
        assert_eq!(back.ids.len(), 4);
        assert_eq!(back.sync(&wide(0), &EmbedSkip::default(), &mut fake).unwrap(), 6, "only what the checkpoint lacks");
        let q = fake(&["passage: n7x\nтело".to_string()]).unwrap().remove(0);
        assert_eq!(back.search_scored(&q, 10), synced(&wide(0)).search_scored(&q, 10));
    }

    #[test]
    fn an_index_owes_rows_to_every_graph_but_the_one_its_last_sync_claimed() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        let (at, moved) = (Stamp { mtime_ns: 1, len: 10 }, Stamp { mtime_ns: 2, len: 10 });
        let mut idx = synced(&graph("x"));
        assert!(idx.behind(at), "a sync nobody claimed a graph for is in line with none");
        idx.synced_against(Some(at));
        idx.save(&store).unwrap();
        let back = DenseIndex::load(&store).unwrap();
        assert!(!back.behind(at), "the claim is read back with the rows");
        assert!(back.behind(moved));
    }

    #[test]
    fn a_checkpoint_owes_rows_even_to_the_graph_its_index_last_claimed() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        let at = Stamp { mtime_ns: 1, len: 10 };
        let mut idx = synced(&wide(0));
        idx.synced_against(Some(at));
        // Another model: every row goes, and the graph `at` names is embedded again from nothing.
        idx.written_by("another/model", 3);
        let err = idx.sync_chunked(&wide(0), &EmbedSkip::default(), &mut fake, ChunkBudget::rows(4), &mut |i, _| {
            i.save(&store)?;
            anyhow::bail!("interrupted")
        }).unwrap_err().to_string();
        assert!(err.contains("interrupted"), "{err}");
        let back = DenseIndex::load(&store).unwrap();
        assert_eq!(back.ids.len(), 4);
        assert!(back.behind(at), "four rows of ten are not the graph it claimed");
    }

    #[test]
    fn a_chunked_sync_over_an_edited_store_leaves_the_same_holes_as_a_plain_one() {
        let mut idx = synced(&wide(0));
        assert_eq!(idx.sync_chunked(&wide(1), &EmbedSkip::default(), &mut fake, ChunkBudget::rows(3), &mut |_, _| Ok(())).unwrap(), 1);
        assert_eq!(idx.free, vec![0]);
        assert_eq!(idx.live, (1..11).collect::<Vec<_>>());
    }

    #[test]
    fn sync_is_sync_chunked_with_one_chunk() {
        let mut plain = DenseIndex::default();
        let n = plain.sync(&wide(0), &EmbedSkip::default(), &mut fake).unwrap();
        let mut chunked = DenseIndex::default();
        let mut seen = Vec::new();
        let m = chunked.sync_chunked(&wide(0), &EmbedSkip::default(), &mut fake, ChunkBudget::rows(usize::MAX),
            &mut |_, p| { seen.push((p.done, p.total)); Ok(()) }).unwrap();
        assert_eq!((m, seen), (n, vec![(n, n)]));
        assert_eq!((chunked.ids, chunked.vectors), (plain.ids, plain.vectors));
    }

    #[test]
    fn sync_embeds_only_changed_nodes_and_drops_removed_ones() {
        let mut idx = DenseIndex::default();
        assert_eq!(idx.sync(&graph("политика"), &EmbedSkip::default(), &mut fake).unwrap(), 2);
        assert_eq!(idx.ids.len(), 2);
        assert_eq!(idx.sync(&graph("политика"), &EmbedSkip::default(), &mut fake).unwrap(), 0);
        assert_eq!(idx.sync(&graph("другое"), &EmbedSkip::default(), &mut fake).unwrap(), 1);
        let mut g = graph("другое");
        g.remove_file("a.md");
        assert_eq!(idx.sync(&g, &EmbedSkip::default(), &mut fake).unwrap(), 0);
        assert!(idx.ids.is_empty() && idx.vectors.is_empty());
    }

    #[test]
    fn owed_counts_the_rows_a_sync_would_embed_and_embeds_none() {
        let mut idx = DenseIndex::default();
        assert_eq!(idx.owed(&wide(0), &EmbedSkip::default()), 10, "an empty index owes every row");
        idx.sync(&wide(0), &EmbedSkip::default(), &mut fake).unwrap();
        assert_eq!(idx.owed(&wide(0), &EmbedSkip::default()), 0);
        assert_eq!(idx.owed(&wide(3), &EmbedSkip::default()), 3);
        assert_eq!(idx.sync(&wide(3), &EmbedSkip::default(), &mut fake).unwrap(), 3, "the count is the sync's own");
    }

    /// A reader out of budget stops its sync after a chunk. What it answers from then is the old
    /// rows and the new ones together, and the sync that finishes later embeds only the rest.
    #[test]
    fn a_stopped_sync_answers_from_old_and_new_rows_and_the_next_embeds_only_the_rest() {
        let mut idx = DenseIndex::default();
        idx.sync(&wide(0), &EmbedSkip::default(), &mut fake).unwrap();
        let stopped = idx.sync_chunked(&wide(10), &EmbedSkip::default(), &mut fake, ChunkBudget::rows(4),
            &mut |_, p| if p.done < p.total { anyhow::bail!("out of budget") } else { Ok(()) });
        assert!(stopped.is_err());
        idx.checkpointed();
        assert_eq!(idx.live.len(), 14, "ten old rows and the four the stopped sync appended");
        assert_eq!(idx.owed(&wide(10), &EmbedSkip::default()), 6);
        assert_eq!(idx.sync(&wide(10), &EmbedSkip::default(), &mut fake).unwrap(), 6);
        assert_eq!(idx.live.len(), 10, "the replaced rows are retired by the sync that finished");
    }

    #[test]
    fn rows_of_another_model_go_before_a_sync_and_the_same_model_keeps_them() {
        let mut idx = synced(&graph("x"));
        idx.written_by(crate::index::embed::UNNAMED_MODEL, 3);
        assert_eq!(idx.ids.len(), 2, "an unnamed store is the small model's and is kept");
        idx.written_by("BAAI/bge-m3", 3);
        assert!(idx.ids.is_empty() && idx.vectors.is_empty() && idx.dim == 0, "another model's rows cannot be appended to");
        assert_eq!(idx.model, "BAAI/bge-m3");
        assert_eq!(idx.sync(&graph("x"), &EmbedSkip::default(), &mut fake).unwrap(), 2);
        idx.written_by("BAAI/bge-m3", 3);
        assert_eq!(idx.ids.len(), 2, "the same model keeps its rows");
    }

    #[test]
    fn a_claim_in_another_case_keeps_the_rows_and_records_its_spelling() {
        let mut idx = synced(&graph("x"));
        idx.written_by("BAAI/BGE-M3", 3);
        assert_eq!(idx.sync(&graph("x"), &EmbedSkip::default(), &mut fake).unwrap(), 2);
        idx.written_by("BAAI/bge-m3", 3);
        assert_eq!(idx.ids.len(), 2, "one model in two cases is not a re-embed");
        assert_eq!(idx.model, "BAAI/bge-m3");
    }

    #[test]
    fn an_unnamed_store_of_another_width_is_emptied_though_the_name_matches() {
        // What `ZEROGRAPH_EMBED_MODEL=<hub id>` + `embed` left behind: another model's rows under
        // no name at all. Claimed by name alone it would keep them, `sync` would match every row
        // by hash and embed nothing, and the store would be recorded as the small model's over
        // rows the small model never wrote — with no later `embed` able to reach it.
        let mut idx = synced(&graph("x"));
        assert_eq!((idx.dim, idx.model.as_str()), (3, ""));
        idx.written_by(crate::index::embed::UNNAMED_MODEL, 3);
        assert_eq!(idx.ids.len(), 2, "the same name at the same width appends");
        idx.written_by(crate::index::embed::UNNAMED_MODEL, 5);
        assert!(idx.ids.is_empty() && idx.vectors.is_empty() && idx.dim == 0,
            "rows of another width cannot be appended to, whatever the name says");
        assert_eq!(idx.sync(&graph("x"), &EmbedSkip::default(), &mut fake_wide).unwrap(), 2);
        assert_eq!(idx.dim, 5);
    }

    #[test]
    fn the_recorded_model_is_read_from_the_metadata_alone() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let mut idx = synced(&graph("x"));
        idx.written_by("BAAI/bge-m3", 3);
        idx.sync(&graph("x"), &EmbedSkip::default(), &mut fake).unwrap();
        idx.save(&store).unwrap();
        assert_eq!(DenseIndex::recorded_model(&store).unwrap().as_deref(), Some("BAAI/bge-m3"));
        assert_eq!(DenseIndex::load(&store).unwrap().model, "BAAI/bge-m3");
    }

    #[test]
    fn a_recorded_name_with_control_characters_is_read_escaped_and_left_as_written() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let hostile = "x\n\u{1b}[31mFORGED\u{7}";
        let mut idx = synced(&graph("x"));
        idx.written_by(hostile, 3);
        idx.sync(&graph("x"), &EmbedSkip::default(), &mut fake).unwrap();
        idx.save(&store).unwrap();
        let read = DenseIndex::recorded_model(&store).unwrap().unwrap();
        assert!(!read.chars().any(char::is_control), "{read:?}");
        assert_eq!(read, "x\\n\\u{1b}[31mFORGED\\u{7}");
        assert_eq!(DenseIndex::load(&store).unwrap().model, hostile, "the file itself is not rewritten by reading it");
    }

    #[test]
    fn a_torn_vectors_file_is_no_index_and_still_names_its_model() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let mut idx = synced(&graph("x"));
        idx.written_by("BAAI/bge-m3", 3);
        idx.sync(&graph("x"), &EmbedSkip::default(), &mut fake).unwrap();
        idx.save(&store).unwrap();
        // A crash between the two writes leaves metadata naming more rows than the file holds.
        // The rows are gone either way; the name must not be, or `ask` reads the store as
        // unnamed, resolves to the configured model, and its resync rebuilds the index under a
        // model the store never chose — a reader moving a store, which cannot happen.
        store.write_atomic("vectors.f32", &[0u8; 4]).unwrap();
        let torn = DenseIndex::load(&store).unwrap();
        assert!(torn.ids.is_empty(), "a torn pair of files is no index at all");
        assert_eq!(torn.model_of_rows().as_deref(), Some("BAAI/bge-m3"));
    }

    #[test]
    fn an_unnamed_store_stays_the_small_model_s_whatever_the_default_names() {
        // The two constants were the same string until the default moved. Were the unnamed rule
        // to follow the default again, every store written before the field existed would be
        // claimed for a model that never wrote it, and re-embedded whole to discover otherwise.
        assert_eq!(crate::index::embed::UNNAMED_MODEL, "intfloat/multilingual-e5-small");
        let mut idx = synced(&graph("x"));
        assert_eq!(idx.model, "");
        idx.written_by(crate::index::embed::UNNAMED_MODEL, 3);
        assert_eq!(idx.ids.len(), 2, "the model that wrote it keeps its rows");
    }

    #[test]
    fn an_unnamed_store_with_rows_reads_as_the_small_model_and_an_empty_one_as_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        assert_eq!(DenseIndex::recorded_model(&store).unwrap(), None, "nothing written yet");
        // Saved without `written_by`, as every store written before the field existed was.
        synced(&graph("x")).save(&store).unwrap();
        assert_eq!(DenseIndex::recorded_model(&store).unwrap().as_deref(), Some(crate::index::embed::UNNAMED_MODEL),
            "an unnamed store holds the small model's rows, so a reader opens the small model");
        let mut idx = synced(&graph("x"));
        idx.written_by("BAAI/bge-m3", 3);
        idx.sync(&graph("x"), &EmbedSkip::default(), &mut fake).unwrap();
        idx.save(&store).unwrap();
        assert_eq!(DenseIndex::recorded_model(&store).unwrap().as_deref(), Some("BAAI/bge-m3"));
    }

    #[test]
    fn search_is_cosine_descending() {
        let idx = synced(&graph("x"));
        let q = fake(&["штраф".to_string()]).unwrap().remove(0);
        assert_eq!(idx.search(&q, 2)[0], "FR-PAY-26");
    }

    #[test]
    fn an_append_leaves_the_surviving_rows_at_their_offsets() {
        let mut idx = synced(&wide(0));
        let before = idx.vectors.clone();
        assert_eq!(idx.sync(&wide(1), &EmbedSkip::default(), &mut fake).unwrap(), 1);
        assert_eq!(idx.free, vec![0]);
        assert_eq!(idx.ids.len(), 11);
        assert_eq!(idx.vectors[idx.dim..before.len()], before[idx.dim..]);
        assert_eq!(idx.ids[10], "FR-W-0");
        assert_eq!(idx.live, (1..11).collect::<Vec<_>>());
    }

    #[test]
    fn a_dead_row_is_never_returned() {
        let mut idx = synced(&wide(0));
        let stale = fake(&["passage: n0x\nтело".to_string()]).unwrap().remove(0);
        idx.sync(&wide(1), &EmbedSkip::default(), &mut fake).unwrap();
        let passages = idx.search(&stale, 20);
        assert_eq!(passages.len(), 10, "one row per live node, the hole scanned by nobody");
        assert!(!passages.iter().any(|id| id.is_empty()));
    }

    #[test]
    fn compaction_fires_past_the_hole_share_and_searches_identically() {
        let mut idx = synced(&wide(0));
        // Three of ten rows die at once: past a quarter of the live rows, so the holes close.
        idx.sync(&wide(3), &EmbedSkip::default(), &mut fake).unwrap();
        assert!(idx.free.is_empty());
        assert_eq!(idx.ids.len(), 10);
        let fresh = synced(&wide(3));
        let q = fake(&["passage: n7x\nтело".to_string()]).unwrap().remove(0);
        assert_eq!(idx.search_scored(&q, 10), fresh.search_scored(&q, 10));
    }

    #[test]
    fn an_append_saves_without_rewriting_the_rows_already_on_disk() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        let mut idx = synced(&wide(0));
        idx.save(&store).unwrap();
        let rows = d.path().join(".zerograph/vectors.f32");
        let before = std::fs::read(&rows).unwrap();
        idx.sync(&wide(1), &EmbedSkip::default(), &mut fake).unwrap();
        idx.save(&store).unwrap();
        let after = std::fs::read(&rows).unwrap();
        assert_eq!(after.len(), before.len() + idx.dim * 4, "one row longer, nothing rewritten");
        assert_eq!(after[..before.len()], before[..]);
        let back = DenseIndex::load(&store).unwrap();
        let q = fake(&["passage: n4x\nтело".to_string()]).unwrap().remove(0);
        assert_eq!(back.free, vec![0]);
        assert_eq!(back.search_scored(&q, 10), idx.search_scored(&q, 10));
    }

    #[test]
    fn a_store_written_before_the_holes_field_loads_unchanged() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        let idx = synced(&graph("x"));
        // Neither `kinds` nor `free`, as the first release wrote it.
        let meta = serde_json::json!({ "ids": idx.ids, "hashes": idx.hashes, "dim": idx.dim });
        store.write_atomic("vectors.json", &serde_json::to_vec(&meta).unwrap()).unwrap();
        store.write_atomic("vectors.f32", &le_bytes(&idx.vectors)).unwrap();
        let back = DenseIndex::load(&store).unwrap();
        assert_eq!(back.kinds, vec![false, false]);
        let q = fake(&["штраф".to_string()]).unwrap().remove(0);
        assert_eq!(back.search(&q, 2)[0], "FR-PAY-26");
    }

    #[test]
    fn a_store_with_no_holes_serialises_without_the_holes_field() {
        let idx = synced(&graph("x"));
        let json = String::from_utf8(serde_json::to_vec(&idx).unwrap()).unwrap();
        assert!(!json.contains("free"), "{json}");
    }

    #[test]
    fn rows_left_by_a_crash_before_the_metadata_rename_are_ignored_then_overwritten() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        let mut idx = synced(&wide(0));
        idx.save(&store).unwrap();
        let rows = d.path().join(".zerograph/vectors.f32");
        let good = std::fs::read(&rows).unwrap();
        // What an append interrupted before its metadata reached disk leaves behind.
        std::fs::write(&rows, [good.clone(), le_bytes(&[9.0, 9.0, 9.0])].concat()).unwrap();
        let mut back = DenseIndex::load(&store).unwrap();
        assert_eq!(back.vectors.len(), 10 * back.dim);
        let q = fake(&["passage: n4x\nтело".to_string()]).unwrap().remove(0);
        assert_eq!(back.search_scored(&q, 10), idx.search_scored(&q, 10));
        back.sync(&wide(1), &EmbedSkip::default(), &mut fake).unwrap();
        back.save(&store).unwrap();
        assert_eq!(std::fs::metadata(&rows).unwrap().len() as usize, good.len() + back.dim * 4);
        assert_eq!(DenseIndex::load(&store).unwrap().ids, back.ids);
    }

    // A store an older binary enriched holds question rows; the probe is one of them exactly, so
    // a search that read it would score 1.0.
    #[test]
    fn a_question_row_an_older_binary_wrote_is_never_returned_and_the_next_sync_retires_it() {
        let mut idx = synced(&graph("x"));
        let mut v = fake(&["query: a".to_string()]).unwrap().remove(0);
        normalise(&mut v);
        idx.ids.push("FR-PAY-22".into());
        idx.hashes.push("q".into());
        idx.kinds.push(true);
        idx.vectors.extend(&v);
        idx.reindex();
        assert!(idx.search_scored(&v, 5).iter().all(|(_, s)| *s < 1.0 - 1e-6));
        assert_eq!(idx.sync(&graph("x"), &EmbedSkip::default(), &mut fake).unwrap(), 0, "nothing to embed");
        assert!(idx.live.iter().all(|&i| !idx.kinds[i]), "the question row is retired");
    }

    #[test]
    fn present_requires_both_files_to_be_non_empty() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        assert!(!DenseIndex::present(&store));
        let mut idx = synced(&graph("x"));
        idx.save(&store).unwrap();
        assert!(DenseIndex::present(&store));
    }

    #[test]
    fn a_query_of_the_wrong_dimension_yields_an_empty_list_not_a_panic() {
        let idx = synced(&graph("x"));
        assert!(idx.search(&[1.0, 2.0], 5).is_empty());
    }

    #[test]
    fn k_larger_than_the_collection_returns_every_row_without_padding() {
        let idx = synced(&graph("x"));
        let q = fake(&["штраф".to_string()]).unwrap().remove(0);
        let passages = idx.search(&q, 1000);
        assert_eq!(passages.len(), 2);
    }

    #[test]
    fn equal_cosine_scores_break_ties_by_id_ascending() {
        let mut g = Graph::default();
        let mut e = Extraction::default();
        // Same label and body embed to the exact same fake vector, so their cosine ties.
        e.node(NodeKind::Requirement, "FR-PAY-99", "same", "z", "a.md", 1);
        e.node(NodeKind::Requirement, "FR-PAY-11", "same", "z", "a.md", 2);
        g.apply(e);
        let idx = synced(&g);
        let q = fake(&["z".to_string()]).unwrap().remove(0);
        let passages = idx.search(&q, 5);
        assert_eq!(passages, vec!["FR-PAY-11".to_string(), "FR-PAY-99".to_string()]);
    }

    #[test]
    fn a_text_file_is_cut_into_rows_the_embedder_reads_whole() {
        let body = format!("{}\n", "a".repeat(99)).repeat(15);
        let rows = text_rows("ops.yaml", &body);
        let head = "passage: ops.yaml\n";
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| r.starts_with(head) && r.chars().count() <= "passage: \n".len() + TEXT_ROW_CHARS));
        assert_eq!(rows.iter().map(|r| &r[head.len()..]).collect::<String>(), body);
    }

    #[test]
    fn a_text_file_with_no_line_break_is_still_cut() {
        let rows = text_rows("blob.txt", &"é".repeat(1_500));
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| r.chars().count() <= "passage: \n".len() + TEXT_ROW_CHARS));
    }

    #[test]
    fn a_blank_text_file_has_no_row() {
        assert!(text_rows("empty.txt", "\n\n  \n").is_empty());
    }

    /// One vector for every text, which `fake` gives none of the test graphs' rows, so a row taken
    /// from the cache is told apart from one embedded here.
    fn constant(texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![1.0, 2.0, 5.0]).collect())
    }

    fn machine_cache() -> (tempfile::TempDir, super::super::cache::VectorCache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = super::super::cache::VectorCache::at(dir.path().join("vectors"), 0).unwrap();
        (dir, cache)
    }

    /// A checkout's `update`: its rows synced under `model` with `embed`, the cache attached.
    fn checkout(cache: &super::super::cache::VectorCache, g: &Graph, model: &str, embed: fn(&[String]) -> Result<Vec<Vec<f32>>>) -> (tempfile::TempDir, DenseIndex, usize, Vec<String>) {
        let repo = tempfile::tempdir().unwrap();
        let store = Store::new(repo.path());
        let mut idx = DenseIndex::load(&store).unwrap();
        idx.use_cache(Some(cache.clone()), "r");
        idx.written_by(model, embed(&["passage: x".into()]).unwrap()[0].len());
        let mut seen = Vec::new();
        let n = idx.sync(g, &EmbedSkip::default(), &mut |t| { seen.extend_from_slice(t); embed(t) }).unwrap();
        idx.save(&store).unwrap();
        (repo, idx, n, seen)
    }

    fn row(idx: &DenseIndex, id: &str) -> Vec<f32> {
        let i = idx.ids.iter().position(|x| x == id).unwrap();
        idx.vectors[i * idx.dim..(i + 1) * idx.dim].to_vec()
    }

    #[test]
    fn rows_another_checkout_embedded_are_taken_from_the_cache_not_embedded() {
        let (_c, cache) = machine_cache();
        checkout(&cache, &graph("x"), "m", constant);
        let (_r, idx, embedded, seen) = checkout(&cache, &graph("отмена другая"), "m", fake);
        assert_eq!(embedded, 1, "only the row the cache does not hold is embedded");
        assert!(seen.len() == 1 && seen[0].contains("отмена другая"), "{seen:?}");
        let mut cached = vec![1.0, 2.0, 5.0];
        normalise(&mut cached);
        assert_eq!(row(&idx, "FR-PAY-26"), cached, "the first checkout's vector, not a fresh embed");
        let held: u64 = cache.status().models.iter().map(|u| u.rows).sum();
        assert_eq!(held, 3, "a row read back from the cache is not written to it again");
    }

    #[test]
    fn rows_the_cache_holds_are_not_counted_as_owed_once_taken() {
        let (_c, cache) = machine_cache();
        checkout(&cache, &graph("x"), "m", constant);
        let g = graph("отмена другая");
        let repo = tempfile::tempdir().unwrap();
        let mut idx = DenseIndex::load(&Store::new(repo.path())).unwrap();
        idx.use_cache(Some(cache), "r");
        idx.written_by("m", 3);
        assert_eq!(idx.owed(&g, &EmbedSkip::default()), 2, "before the lookup every row is owed");
        assert_eq!(idx.take_cached(&g, &EmbedSkip::default()), 1);
        assert_eq!(idx.owed(&g, &EmbedSkip::default()), 1, "only the row the cache lacks");
        let mut seen = Vec::new();
        let n = idx.sync(&g, &EmbedSkip::default(), &mut |t| { seen.extend_from_slice(t); fake(t) }).unwrap();
        assert_eq!((n, seen.len()), (1, 1), "the sync embeds exactly what the count said");
        assert_eq!(idx.owed(&g, &EmbedSkip::default()), 0);
        assert_eq!(idx.live.len(), 2, "the cached row is kept, not duplicated");
    }

    #[test]
    fn a_bounded_reader_lends_what_its_shards_hold_and_embeds_only_the_rest() {
        let (_c, cache) = machine_cache();
        let g = wide(0);
        let reader = |cache: &super::super::cache::VectorCache| {
            let repo = tempfile::tempdir().unwrap();
            let mut idx = DenseIndex::load(&Store::new(repo.path())).unwrap();
            idx.use_cache(Some(cache.clone()), "r");
            idx.bound_cache(1);
            idx.written_by("m", 3);
            let lent = idx.take_cached(&g, &EmbedSkip::default());
            let mut embedded = 0;
            idx.sync(&g, &EmbedSkip::default(), &mut |t| { embedded += t.len(); constant(t) }).unwrap();
            (repo, lent, embedded)
        };
        let (_r, lent, cold) = reader(&cache);
        assert_eq!(lent, 0, "a cold cache lends nothing");
        let rows = cold;
        assert!(rows > 1, "the graph spans more shards than the bound");
        let (_r, lent, embedded) = reader(&cache);
        assert!(lent >= 1 && lent < rows, "a bound of one shard lends that shard's rows alone: {lent} of {rows}");
        assert_eq!(lent + embedded, rows, "and the rest are embedded, not lost");
    }

    #[test]
    fn a_row_cached_under_another_revision_is_never_taken() {
        let (_c, cache) = machine_cache();
        checkout(&cache, &graph("x"), "m", constant);
        let g = graph("x");
        let repo = tempfile::tempdir().unwrap();
        let mut idx = DenseIndex::load(&Store::new(repo.path())).unwrap();
        idx.use_cache(Some(cache), "another-commit");
        idx.written_by("m", 3);
        assert_eq!(idx.take_cached(&g, &EmbedSkip::default()), 0);
    }

    #[test]
    fn a_row_of_another_model_or_width_is_never_taken() {
        let (_c, cache) = machine_cache();
        checkout(&cache, &graph("x"), "another/model", fake);
        checkout(&cache, &graph("x"), "m", fake_wide);
        assert_eq!(checkout(&cache, &graph("x"), "m", fake).2, 2, "neither the other name nor the other width crosses");
        assert_eq!(checkout(&cache, &graph("x"), "M", fake).2, 0, "a hub id names one model in any case");
    }

    #[test]
    fn only_rows_this_process_embedded_enter_the_cache() {
        let (_c, cache) = machine_cache();
        // A store whose rows came with the checkout — a committed `.zerograph`, say: synced with
        // nothing to embed, it writes nothing through.
        let repo = tempfile::tempdir().unwrap();
        let store = Store::new(repo.path());
        let mut planted = DenseIndex::load(&store).unwrap();
        planted.written_by("m", 3);
        planted.sync(&graph("x"), &EmbedSkip::default(), &mut constant).unwrap();
        planted.save(&store).unwrap();
        let mut idx = DenseIndex::load(&store).unwrap();
        idx.use_cache(Some(cache.clone()), "r");
        idx.written_by("m", 3);
        assert_eq!(idx.sync(&graph("x"), &EmbedSkip::default(), &mut fake).unwrap(), 0);
        assert!(cache.status().models.is_empty(), "a store's own rows never reach the cache");
        assert_eq!(checkout(&cache, &graph("x"), "m", fake).2, 2, "so another checkout embeds them itself");
    }

    #[test]
    fn a_sync_that_claimed_no_model_neither_reads_nor_writes_the_cache() {
        let (_c, cache) = machine_cache();
        checkout(&cache, &graph("x"), crate::index::embed::UNNAMED_MODEL, fake);
        let before = cache.status();
        let repo = tempfile::tempdir().unwrap();
        let mut idx = DenseIndex::load(&Store::new(repo.path())).unwrap();
        idx.use_cache(Some(cache.clone()), "r");
        assert_eq!(idx.sync(&graph("отмена другая"), &EmbedSkip::default(), &mut fake).unwrap(), 2);
        assert_eq!(cache.status(), before);
    }

    #[test]
    fn a_checkpoint_keeps_the_cached_rows_beside_the_embedded_ones() {
        let (_c, cache) = machine_cache();
        checkout(&cache, &graph("x"), "m", constant);
        let repo = tempfile::tempdir().unwrap();
        let store = Store::new(repo.path());
        let mut idx = DenseIndex::load(&store).unwrap();
        idx.use_cache(Some(cache), "r");
        idx.written_by("m", 3);
        let mut saves = 0;
        let n = idx.sync_chunked(&graph("отмена другая"), &EmbedSkip::default(), &mut fake, ChunkBudget::rows(1), &mut |idx, p| {
            saves += 1;
            assert_eq!((p.done, p.total), (1, 1), "progress counts the embeds alone");
            idx.save(&store)
        }).unwrap();
        assert_eq!((n, saves), (1, 1));
        let saved = DenseIndex::load(&store).unwrap();
        assert_eq!(saved.owed(&graph("отмена другая"), &EmbedSkip::default()), 0, "the checkpoint holds both rows");
    }
}
