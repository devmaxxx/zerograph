//! Vectors this user embedded on this machine, keyed by what a vector is a function of: the
//! model, the commit its files were fetched at, its width and the row's text. A cold build is
//! almost all embedding, and a second checkout of the same code — a worktree, a clone, a fork —
//! embeds the same texts again, so a row any of them embedded once is a read for the rest.
//!
//! One directory per model, revision, width and format, 4096 append-only shard files in each,
//! picked by the first twelve bits of the row's hash. A reader reads every shard a wanted row
//! falls in whole, so the count keeps a shard near a megabyte at the default cap: an edit owing a
//! few rows reads a few megabytes, not a sixteenth of the cache. A shard is a header naming its
//! model, revision and width, then fixed-width records, so the stride is the index and there is
//! no second file to tear. Readers take no lock: they count whole records and check each one, so
//! a record mid-append, cut short by a crash, or corrupted on disk is skipped, never returned.
//! Writers append under a lock of the shard's own, held for one write.
//!
//! Only `DenseIndex::sync_chunked`'s embed loop writes here. A store's `vectors.f32` is never
//! imported, so rows a branch committed under `.zerograph/` cannot seed it.

use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"ZGVCACHE";
/// Bumped whenever a record's layout, or what its vector is made from beyond the model and the
/// text (prefixes, pooling, normalisation), changes. Each value has directories of its own.
const VERSION: u32 = 2;
const KEY: usize = 32;
const USED: usize = 4;
const CHECK: usize = 8;

/// A few GB: every model's rows of every checkout a laptop is likely to hold, and well short of
/// what anyone would notice missing from a disk.
pub const DEFAULT_MAX: u64 = 4 << 30;

/// A hit rewrites its record's `used` only once it is this many minutes stale. Recency within the
/// hour is no information an eviction at gigabyte scale needs, and a reader answering every few
/// seconds would otherwise write on every answer.
const TOUCH_AFTER: u32 = 60;

/// Over the cap, a writer prunes to this share of it, so the next few writes do not prune again.
const PRUNE_TO_PERCENT: u64 = 80;

/// A writer weighs the whole cache against its cap only when a shard it appended to crosses a
/// multiple of this. Weighing means a stat of every shard of every model, which a reader paying
/// a few rows after an edit would otherwise add to every answer; the cap is overrun by at most
/// this much per shard in between.
const WEIGH_EVERY: u64 = 64 << 10;

#[derive(Debug, Clone)]
pub struct VectorCache { root: PathBuf, max: u64, detached: bool }

/// One model's directory as `status` reads it.
#[derive(Debug, PartialEq)]
pub struct Usage { pub model: String, pub revision: String, pub dims: usize, pub rows: u64, pub bytes: u64, pub current: bool }

/// What `status` read: each model's directory, and the bytes of shards this build cannot
/// describe — a later format, or no header at all — which the cap still weighs.
#[derive(Debug, PartialEq)]
pub struct Status { pub models: Vec<Usage>, pub other: u64 }

impl Status {
    /// Every byte the cap is weighed against, as `bytes` counts them.
    pub fn bytes(&self) -> u64 { self.models.iter().map(|u| u.bytes).sum::<u64>() + self.other }
}

/// What a prune dropped, by why: past the cap (`evicted`), a second record of a key or one no
/// reader returns (`repeated`), or a shard of an earlier format (`older`).
#[derive(Debug, Default, PartialEq)]
pub struct Pruned { pub evicted: u64, pub repeated: u64, pub older: u64, pub failed: u64, pub before: u64, pub after: u64 }

impl Pruned {
    pub fn rows(&self) -> u64 { self.evicted + self.repeated + self.older }
}

/// `$ZEROGRAPH_VECTOR_CACHE`, else `%LOCALAPPDATA%\zerograph\vectors` on Windows and
/// `~/.cache/zerograph/vectors` elsewhere. A relative override is refused: it would put the cache
/// wherever each process happens to stand. So is one on Unix under a directory another user can
/// change.
pub fn default_root() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let platform = local.as_ref().map(|p| p.join("zerograph").join("vectors"));
        // Only Unix checks the root's owner and mode. On Windows an override elsewhere could be
        // a folder another account made first and can write, so it must sit in the user's own
        // profile.
        root_from(crate::env_os("VECTOR_CACHE"), platform, local.as_deref())
    }
    #[cfg(not(windows))]
    {
        let platform = std::env::home_dir().map(|h| h.join(".cache").join("zerograph").join("vectors"));
        let named = crate::env_os("VECTOR_CACHE").filter(|v| !v.is_empty());
        let overridden = named.is_some();
        let root = root_from(named, platform, None)?;
        // The platform root sits in the user's home; an override can sit anywhere.
        match overridden {
            true => crate::store::refuse_shared_ancestors(&root),
            false => Ok(root),
        }
    }
}

fn root_from(named: Option<std::ffi::OsString>, platform: Option<PathBuf>, within: Option<&Path>) -> Result<PathBuf> {
    let Some(named) = named.filter(|v| !v.is_empty()).map(PathBuf::from) else {
        return platform.filter(|p| p.is_absolute())
            .context("no vector cache directory: this environment names no home directory");
    };
    if !named.is_absolute() {
        anyhow::bail!("ZEROGRAPH_VECTOR_CACHE must be an absolute path, not {}", named.display());
    }
    if let Some(base) = within {
        let climbs = named.components().any(|c| matches!(c, std::path::Component::ParentDir));
        if climbs || !named.starts_with(base) {
            anyhow::bail!("ZEROGRAPH_VECTOR_CACHE must sit under {}, not {}", base.display(), named.display());
        }
    }
    Ok(named)
}

/// Said once per process: a cache that cannot be used costs embeds, not answers, and a resident
/// reader would otherwise say so on every one.
pub(crate) fn warn_once(what: &str, err: &anyhow::Error) {
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!("vector cache: {what}, embedding without it ({err:#})");
    }
}

fn now_minutes() -> u32 {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    u32::try_from(secs / 60).unwrap_or(u32::MAX)
}

fn key_of(hex: &str) -> Option<[u8; KEY]> {
    if hex.len() != KEY * 2 { return None; }
    let mut out = [0u8; KEY];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

fn hex_of(key: &[u8]) -> String { key.iter().map(|b| format!("{b:02x}")).collect() }

fn check_of(key: &[u8], row: &[u8]) -> [u8; CHECK] {
    let mut h = blake3::Hasher::new();
    h.update(key);
    h.update(row);
    let mut out = [0u8; CHECK];
    out.copy_from_slice(&h.finalize().as_bytes()[..CHECK]);
    out
}

/// What a shard of one model at one revision and width looks like. A hub id names one model in
/// any case, as `DenseIndex::written_by` has it, so the id is held lower-cased. The revision is
/// the commit the model's files were fetched at: a model that follows the hub's `main` gives other
/// vectors once its weights move, under the same id and width.
struct Layout { model: String, revision: String, dims: usize }

impl Layout {
    fn new(model: &str, revision: &str, dims: usize) -> Layout {
        Layout { model: model.to_ascii_lowercase(), revision: revision.to_string(), dims }
    }

    fn header(&self) -> Vec<u8> {
        let mut h = Vec::with_capacity(24 + self.model.len() + self.revision.len());
        h.extend_from_slice(MAGIC);
        h.extend_from_slice(&VERSION.to_le_bytes());
        h.extend_from_slice(&(self.dims as u32).to_le_bytes());
        h.extend_from_slice(&(self.model.len() as u32).to_le_bytes());
        h.extend_from_slice(self.model.as_bytes());
        h.extend_from_slice(&(self.revision.len() as u32).to_le_bytes());
        h.extend_from_slice(self.revision.as_bytes());
        h
    }

    fn row_len(&self) -> usize { self.dims * 4 }
    fn record_len(&self) -> usize { KEY + USED + self.row_len() + CHECK }

    /// The directory name: readable, and unique per model, revision and format by the hash and
    /// the suffix, so two ids that slug alike never share shards and a format this build cannot
    /// read is never appended to. The header is checked on every read all the same.
    fn dir_name(&self) -> String {
        let slug: String = self.model.chars().take(64)
            .map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect();
        let mut h = blake3::Hasher::new();
        h.update(self.model.as_bytes());
        h.update(&[0]);
        h.update(self.revision.as_bytes());
        let tag = &h.finalize().to_hex()[..8];
        format!("{slug}-{tag}@{}.v{VERSION}", self.dims)
    }

    fn record(&self, key: &[u8; KEY], row: &[f32], used: u32) -> Vec<u8> {
        let mut r = Vec::with_capacity(self.record_len());
        r.extend_from_slice(key);
        r.extend_from_slice(&used.to_le_bytes());
        for x in row { r.extend_from_slice(&x.to_le_bytes()); }
        let check = check_of(key, &r[KEY + USED..]);
        r.extend_from_slice(&check);
        r
    }

    /// The row a whole record holds, or `None` when its check fails or a float is not finite:
    /// the only rows a reader may return.
    fn row_of(&self, rec: &[u8]) -> Option<Vec<f32>> {
        let body = &rec[KEY + USED..KEY + USED + self.row_len()];
        if rec[KEY + USED + self.row_len()..] != check_of(&rec[..KEY], body) { return None; }
        #[allow(clippy::chunks_exact_to_as_chunks)]
        let row: Vec<f32> = body.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        row.iter().all(|x| x.is_finite()).then_some(row)
    }

    fn used_of(rec: &[u8]) -> u32 { u32::from_le_bytes([rec[KEY], rec[KEY + 1], rec[KEY + 2], rec[KEY + 3]]) }

    /// The whole records of a shard's bytes, or none when the header is not this layout's.
    fn records<'a>(&self, bytes: &'a [u8]) -> impl Iterator<Item = (usize, &'a [u8])> + 'a {
        let header = self.header();
        let rl = self.record_len();
        let body = match bytes.starts_with(&header) { true => &bytes[header.len()..], false => &[][..] };
        let base = header.len();
        body.chunks_exact(rl).enumerate().map(move |(i, r)| (base + i * rl, r))
    }
}

/// What a shard's header says, for `status` and `prune`, which have no layout to compare with.
enum Header {
    Current(Layout),
    /// Written by an earlier format, whose rows this build never reads: `prune` drops them whole.
    Older { model: String, dims: usize, rows: u64 },
    /// Written by a later format: left alone, since a newer build on this machine still reads it.
    Newer,
    /// No header at all, or one cut short: nothing anyone can read.
    Garbage,
}

fn word_at(bytes: &[u8], at: usize) -> Option<u32> {
    bytes.get(at..at + 4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
}

fn read_header(bytes: &[u8]) -> Header {
    let str_at = |at: usize, len: usize| bytes.get(at..at + len).and_then(|b| std::str::from_utf8(b).ok());
    if !bytes.starts_with(MAGIC) { return Header::Garbage; }
    let (Some(version), Some(dims), Some(len)) = (word_at(bytes, 8), word_at(bytes, 12), word_at(bytes, 16)) else { return Header::Garbage };
    let (dims, len) = (dims as usize, len as usize);
    if version > VERSION { return Header::Newer; }
    let Some(model) = str_at(20, len).filter(|_| dims > 0) else { return Header::Garbage };
    if version < VERSION {
        // Format 1 ended its header at the model's id and had no revision.
        let rows = (bytes.len() - (20 + len)) / (KEY + USED + dims * 4 + CHECK);
        return Header::Older { model: model.to_ascii_lowercase(), dims, rows: rows as u64 };
    }
    let Some(rev_len) = word_at(bytes, 20 + len) else { return Header::Garbage };
    let Some(revision) = str_at(24 + len, rev_len as usize) else { return Header::Garbage };
    Header::Current(Layout::new(model, revision, dims))
}

fn shard_of(key: &[u8; KEY]) -> String { format!("{:02x}{:x}", key[0], key[1] >> 4) }

fn open(path: &Path, write: bool, create: bool) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true).write(write).create(create);
    crate::store::open_unlinked(&mut o, path)
}

fn read_all(f: &mut std::fs::File) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(f.metadata().map_or(0, |m| m.len() as usize));
    f.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(unix)]
fn write_at(f: &std::fs::File, at: u64, bytes: &[u8]) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(f, bytes, at)
}

#[cfg(windows)]
fn write_at(f: &std::fs::File, at: u64, bytes: &[u8]) -> std::io::Result<()> {
    let mut done = 0;
    while done < bytes.len() {
        done += std::os::windows::fs::FileExt::seek_write(f, &bytes[done..], at + done as u64)?;
    }
    Ok(())
}

/// A shard's lock, held for as long as the guard lives. A file of its own rather than the shard,
/// since `prune` renames a new shard over the old and a writer waiting on the old one's lock would
/// then append to a file nobody reads.
fn lock_shard(dir: &Path, shard: &str) -> Result<std::fs::File> {
    let path = dir.join(format!("{shard}.lock"));
    let f = open(&path, true, true).with_context(|| format!("open {}", path.display()))?;
    f.lock().with_context(|| format!("lock {}", path.display()))?;
    Ok(f)
}

impl VectorCache {
    /// The cache at `default_root`, or `None` when `max` turns it off, no root can be named, or
    /// the root is not private to this user — each of which costs embeds and nothing else. Unit
    /// tests never reach the machine's cache: they make their own with `at`.
    /// Its prunes run in a process of their own: the write that takes the cache over its cap can
    /// be a reader's, and a prune rewrites gigabytes.
    pub fn machine(max: u64) -> Option<VectorCache> {
        if max == 0 || cfg!(test) { return None; }
        match default_root().and_then(|root| VectorCache::at(root, max)) {
            Ok(c) => Some(VectorCache { detached: true, ..c }),
            Err(err) => { warn_once("not used", &err); None }
        }
    }

    /// The cache at `root`, which is made private to this user or refused. Its writes prune in
    /// place.
    pub fn at(root: PathBuf, max: u64) -> Result<VectorCache> {
        crate::store::prepare_private_dir(&root)?;
        Ok(VectorCache { root, max, detached: false })
    }

    /// The cache at `root` for a report, which must not make the directory it reports on: one
    /// that is not there yet is an empty cache, and `at` takes over once one stands.
    pub fn for_status(root: PathBuf, max: u64) -> Result<VectorCache> {
        if root.exists() { return VectorCache::at(root, max); }
        Ok(VectorCache { root, max, detached: false })
    }

    /// What a prune trims to: under the cap by enough that the next few writes do not prune again,
    /// and to nothing when the cap is 0.
    pub fn prune_target(&self) -> u64 { self.max / 100 * PRUNE_TO_PERCENT }

    pub fn root(&self) -> &Path { &self.root }
    pub fn max(&self) -> u64 { self.max }

    /// Rows for as many of the `wanted` hex text hashes as the cache holds under `model` at
    /// `revision` and `dims`.
    pub fn get(&self, model: &str, revision: &str, dims: usize, wanted: &[&str]) -> HashMap<String, Vec<f32>> {
        self.get_at(model, revision, dims, wanted, None, now_minutes())
    }

    /// `get` over at most `shards` of the shards the wanted rows fall in, those holding the most
    /// of them first. Each shard is read whole, so a reader that owes a pull's worth of rows would
    /// otherwise read most of the cache before its budget is weighed.
    pub fn get_within(&self, model: &str, revision: &str, dims: usize, wanted: &[&str], shards: usize) -> HashMap<String, Vec<f32>> {
        self.get_at(model, revision, dims, wanted, Some(shards), now_minutes())
    }

    fn get_at(&self, model: &str, revision: &str, dims: usize, wanted: &[&str], shards: Option<usize>, now: u32) -> HashMap<String, Vec<f32>> {
        let mut found = HashMap::new();
        if dims == 0 || model.is_empty() { return found; }
        let layout = Layout::new(model, revision, dims);
        let dir = self.root.join(layout.dir_name());
        let mut by_shard: HashMap<String, HashSet<[u8; KEY]>> = HashMap::new();
        for k in wanted.iter().filter_map(|h| key_of(h)) { by_shard.entry(shard_of(&k)).or_default().insert(k); }
        let mut by_shard: Vec<(String, HashSet<[u8; KEY]>)> = by_shard.into_iter().collect();
        by_shard.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
        by_shard.truncate(shards.unwrap_or(usize::MAX));
        for (shard, mut keys) in by_shard {
            let path = dir.join(format!("{shard}.rows"));
            // Read-write for the touch below; a shard that will not open so still reads.
            let Ok(mut f) = open(&path, true, false).or_else(|_| open(&path, false, false)) else { continue };
            let Ok(bytes) = read_all(&mut f) else { continue };
            let mut stale = Vec::new();
            for (at, rec) in layout.records(&bytes) {
                let key: &[u8; KEY] = rec[..KEY].try_into().expect("a record starts with its key");
                if !keys.contains(key) { continue; }
                let Some(row) = layout.row_of(rec) else { continue };
                keys.remove(key);
                if now.saturating_sub(Layout::used_of(rec)) > TOUCH_AFTER { stale.push(at); }
                found.insert(hex_of(key), row);
                if keys.is_empty() { break; }
            }
            // `used` is outside the check, so a touch racing a reader cannot make a record invalid,
            // and one lost to a prune's rename only leaves the row a little older.
            for at in stale { let _ = write_at(&f, (at + KEY) as u64, &now.to_le_bytes()); }
        }
        found
    }

    /// Appends `rows` — hex text hash and vector — under `model` at `revision` and `dims`, then
    /// prunes, or starts a prune, when the cache is over its cap. A row of another width or with a
    /// non-finite float is not stored, and a text hash given twice is stored once.
    pub fn put(&self, model: &str, revision: &str, dims: usize, rows: &[(&str, &[f32])]) -> Result<()> {
        self.put_at(model, revision, dims, rows, now_minutes())
    }

    fn put_at(&self, model: &str, revision: &str, dims: usize, rows: &[(&str, &[f32])], now: u32) -> Result<()> {
        if dims == 0 || model.is_empty() { return Ok(()); }
        let layout = Layout::new(model, revision, dims);
        let mut by_shard: HashMap<String, Vec<u8>> = HashMap::new();
        // Two nodes with the same text are embedded once each and handed over together; a second
        // record of one key is bytes no reader returns until a prune drops them.
        let mut seen = HashSet::new();
        for (hash, row) in rows {
            let Some(key) = key_of(hash) else { continue };
            if row.len() != dims || !row.iter().all(|x| x.is_finite()) || !seen.insert(key) { continue; }
            by_shard.entry(shard_of(&key)).or_default().extend(layout.record(&key, row, now));
        }
        if by_shard.is_empty() { return Ok(()); }
        let dir = self.root.join(layout.dir_name());
        make_dir(&dir)?;
        let mut crossed = false;
        for (shard, records) in by_shard {
            let (from, to) = append(&dir, &shard, &layout, &records)?;
            crossed |= from / WEIGH_EVERY != to / WEIGH_EVERY;
        }
        if crossed && self.max > 0 && self.bytes() > self.max {
            match self.detached {
                true => prune_detached(&self.root),
                false => { self.prune(self.prune_target())?; }
            }
        }
        Ok(())
    }

    fn model_dirs(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(&self.root) else { return Vec::new() };
        let mut dirs: Vec<PathBuf> = entries.flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path()).collect();
        dirs.sort();
        dirs
    }

    fn shards(dir: &Path) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut shards: Vec<PathBuf> = entries.flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "rows")).collect();
        shards.sort();
        shards
    }

    /// Bytes on disk across every model: what the cap is weighed against.
    pub fn bytes(&self) -> u64 {
        self.model_dirs().iter().flat_map(|d| Self::shards(d))
            .filter_map(|p| std::fs::symlink_metadata(p).ok()).map(|m| m.len()).sum()
    }

    /// Rows a reader would return and bytes on disk, per model and revision. Every shard is read
    /// whole, so a torn, corrupt or repeated record is not counted as a row; its bytes still are.
    /// A directory of an earlier format is listed with `current` false and its whole records as
    /// rows: nothing reads them, and the next prune drops them.
    pub fn status(&self) -> Status {
        let mut out = Vec::new();
        let mut other = 0;
        for dir in self.model_dirs() {
            let mut usage: Option<Usage> = None;
            for shard in Self::shards(&dir) {
                let Ok(bytes) = open(&shard, false, false).and_then(|mut f| read_all(&mut f)) else {
                    other += std::fs::symlink_metadata(&shard).map_or(0, |m| m.len());
                    continue;
                };
                let (model, revision, dims, rows, current) = match read_header(&bytes) {
                    Header::Current(layout) => {
                        let mut keys = HashSet::new();
                        let rows = layout.records(&bytes)
                            .filter(|(_, rec)| layout.row_of(rec).is_some() && keys.insert(&rec[..KEY]))
                            .count() as u64;
                        (layout.model, layout.revision, layout.dims, rows, true)
                    }
                    Header::Older { model, dims, rows } => (model, String::new(), dims, rows, false),
                    Header::Newer | Header::Garbage => { other += bytes.len() as u64; continue }
                };
                let u = usage.get_or_insert(Usage { model, revision, dims, rows: 0, bytes: 0, current });
                u.rows += rows;
                u.bytes += bytes.len() as u64;
            }
            out.extend(usage);
        }
        Status { models: out, other }
    }

    /// Trims the cache to `target` bytes, least recently used first. On the way it drops every
    /// torn, corrupt or repeated record, and every shard of an earlier format, which no reader of
    /// this build returns. A shard of a later format is left as it is, and so is one this pass
    /// cannot rewrite: it is counted in `failed` and the pass goes on. A prune already running
    /// elsewhere is left to it.
    pub fn prune(&self, target: u64) -> Result<Pruned> {
        let lock_path = self.root.join("prune.lock");
        let lock = open(&lock_path, true, true).with_context(|| format!("open {}", lock_path.display()))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(Pruned::default()),
            Err(std::fs::TryLockError::Error(e)) => return Err(e).with_context(|| format!("lock {}", lock_path.display())),
        }
        let before = self.bytes();
        // The cut is found over every model at once: recency is the only order, whoever wrote it.
        // What stays whatever the cut — headers, and shards of a later format — is taken off the
        // target first.
        let mut uses: Vec<(u32, u64)> = Vec::new();
        let mut fixed = 0u64;
        for dir in self.model_dirs() {
            for shard in Self::shards(&dir) {
                let Ok(bytes) = open(&shard, false, false).and_then(|mut f| read_all(&mut f)) else { continue };
                match read_header(&bytes) {
                    Header::Current(layout) => {
                        fixed += layout.header().len() as u64;
                        let rl = layout.record_len() as u64;
                        uses.extend(layout.records(&bytes).filter(|(_, r)| layout.row_of(r).is_some()).map(|(_, r)| (Layout::used_of(r), rl)));
                    }
                    Header::Newer => fixed += bytes.len() as u64,
                    Header::Older { .. } | Header::Garbage => {}
                }
            }
        }
        let cut = Cut::new(uses, target.saturating_sub(fixed));
        let mut out = Pruned { before, ..Pruned::default() };
        let mut ties = cut.ties;
        for dir in self.model_dirs() {
            for shard in Self::shards(&dir) {
                let Some(name) = shard.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
                if let Err(err) = self.prune_shard(&dir, &shard, &name, &cut, &mut ties, &mut out) {
                    out.failed += 1;
                    if out.failed == 1 { eprintln!("cache: prune left {} as it was ({err:#})", shard.display()); }
                }
            }
        }
        drop(lock);
        out.after = self.bytes();
        Ok(out)
    }

    fn prune_shard(&self, dir: &Path, shard: &Path, name: &str, cut: &Cut, ties: &mut u64, out: &mut Pruned) -> Result<()> {
        let _guard = lock_shard(dir, name)?;
        let Ok(bytes) = open(shard, false, false).and_then(|mut f| read_all(&mut f)) else { return Ok(()) };
        let layout = match read_header(&bytes) {
            Header::Current(layout) => layout,
            Header::Newer => return Ok(()),
            Header::Older { rows, .. } => {
                std::fs::remove_file(shard).with_context(|| format!("remove {}", shard.display()))?;
                out.older += rows;
                return Ok(());
            }
            Header::Garbage => return std::fs::remove_file(shard).with_context(|| format!("remove {}", shard.display())),
        };
        let header = layout.header();
        let mut kept = header.clone();
        let mut seen = HashSet::new();
        let (mut repeated, mut evicted) = (0u64, 0u64);
        for (_, rec) in layout.records(&bytes) {
            if layout.row_of(rec).is_none() || !seen.insert(rec[..KEY].to_vec()) { repeated += 1; continue; }
            let used = Layout::used_of(rec);
            let keep = match cut.at {
                None => true,
                Some(at) if used > at => true,
                Some(at) if used == at && *ties >= rec.len() as u64 => { *ties -= rec.len() as u64; true }
                Some(_) => false,
            };
            match keep {
                true => kept.extend_from_slice(rec),
                false => evicted += 1,
            }
        }
        if kept.len() == bytes.len() { return Ok(()); }
        if kept.len() == header.len() {
            std::fs::remove_file(shard).with_context(|| format!("remove {}", shard.display()))?;
        } else {
            let tmp = dir.join(format!("{name}.rows.tmp"));
            let _ = std::fs::remove_file(&tmp);
            let mut f = std::fs::OpenOptions::new();
            f.write(true).create_new(true);
            let mut f = crate::store::open_unlinked(&mut f, &tmp).with_context(|| format!("create {}", tmp.display()))?;
            f.write_all(&kept).with_context(|| format!("write {}", tmp.display()))?;
            drop(f);
            if let Err(e) = crate::store::rename_over(&tmp, shard) {
                let _ = std::fs::remove_file(&tmp);
                return Err(e).with_context(|| format!("replace {}", shard.display()));
            }
        }
        // Counted once the shard is rewritten, so a shard left as it was reports nothing dropped.
        out.repeated += repeated;
        out.evicted += evicted;
        Ok(())
    }
}

/// Where a prune cuts: records used after `at` stay, and of those used exactly at `at`, as many
/// as `ties` bytes still allow — so a cache filled within one minute is trimmed, not emptied.
struct Cut { at: Option<u32>, ties: u64 }

impl Cut {
    fn new(mut uses: Vec<(u32, u64)>, target: u64) -> Cut {
        uses.sort_by_key(|u| std::cmp::Reverse(u.0));
        let mut kept = 0u64;
        for (i, &(used, len)) in uses.iter().enumerate() {
            if kept + len > target {
                let above: u64 = uses[..i].iter().filter(|u| u.0 > used).map(|u| u.1).sum();
                return Cut { at: Some(used), ties: target.saturating_sub(above) };
            }
            kept += len;
        }
        Cut { at: None, ties: 0 }
    }
}

/// How long a process waits before it starts another background prune. A resident reader over
/// the cap would otherwise start one on every write until the first had finished; one that
/// starts none again stays over the cap for as long as it lives.
const PRUNE_EVERY: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// `zerograph cache prune` in a background process, at most once per `PRUNE_EVERY`.
fn prune_detached(root: &Path) {
    static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    if !due(&mut LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner), std::time::Instant::now()) { return; }
    if let Err(err) = crate::refresh::spawn_quiet(root, &["cache", "prune"]) {
        warn_once("over its cap and not pruned", &err);
    }
}

/// Whether a prune last started at `last` may start again at `now`, recording it when it may.
fn due(last: &mut Option<std::time::Instant>, now: std::time::Instant) -> bool {
    if last.is_some_and(|at| now.saturating_duration_since(at) < PRUNE_EVERY) { return false; }
    *last = Some(now);
    true
}

/// A model's directory under the root, made private and never a link.
fn make_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    let made = { use std::os::unix::fs::DirBuilderExt; std::fs::DirBuilder::new().mode(0o700).create(dir) };
    #[cfg(not(unix))]
    let made = std::fs::create_dir(dir);
    match made {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e).with_context(|| format!("create {}", dir.display())),
    }
    let meta = std::fs::symlink_metadata(dir).with_context(|| format!("stat {}", dir.display()))?;
    if !meta.is_dir() { anyhow::bail!("{} is not a directory", dir.display()); }
    Ok(())
}

/// `records` appended to one shard under its lock, less those whose key the shard already holds:
/// another checkout may have embedded the same text since this one looked. A held record is checked
/// only when its key is one being appended, which keeps the work under the lock to a scan; one
/// that fails its check holds nothing, so the new record is appended after it and a reader, which
/// skips a record that fails, finds that one.
/// A shard that does not start with this layout's header — new or garbage — is started over; a
/// torn tail a crash left is cut back to the last whole record first, or every record after it
/// would be read out of step. A shard whose header names another format is left alone and nothing
/// is appended: it is not this build's to rewrite. Returns the shard's length before and after.
fn append(dir: &Path, shard: &str, layout: &Layout, records: &[u8]) -> Result<(u64, u64)> {
    let _guard = lock_shard(dir, shard)?;
    let path = dir.join(format!("{shard}.rows"));
    let mut f = open(&path, true, true).with_context(|| format!("open {}", path.display()))?;
    let header = layout.header();
    let len = f.metadata().with_context(|| format!("stat {}", path.display()))?.len();
    let mut head = vec![0u8; header.len().min(len as usize)];
    let read = f.read_exact(&mut head).is_ok();
    if read && head.starts_with(MAGIC) && word_at(&head, 8).is_some_and(|v| v != VERSION) { return Ok((len, len)); }
    let fits = read && len >= header.len() as u64 && head == header;
    let at = match fits {
        true => {
            let whole = (len - header.len() as u64) / layout.record_len() as u64;
            header.len() as u64 + whole * layout.record_len() as u64
        }
        false => {
            write_at(&f, 0, &header).with_context(|| format!("write {}", path.display()))?;
            header.len() as u64
        }
    };
    if at != len { f.set_len(at).with_context(|| format!("truncate {}", path.display()))?; }
    let fresh = match fits {
        true => {
            f.seek(std::io::SeekFrom::Start(0)).with_context(|| format!("seek {}", path.display()))?;
            let bytes = read_all(&mut f).with_context(|| format!("read {}", path.display()))?;
            let mut new: HashSet<&[u8]> = records.chunks(layout.record_len()).map(|rec| &rec[..KEY]).collect();
            for (_, rec) in layout.records(&bytes) {
                if new.is_empty() { break; }
                if new.contains(&rec[..KEY]) && layout.row_of(rec).is_some() { new.remove(&rec[..KEY]); }
            }
            records.chunks(layout.record_len()).filter(|rec| new.contains(&rec[..KEY])).flatten().copied().collect()
        }
        false => records.to_vec(),
    };
    write_at(&f, at, &fresh).with_context(|| format!("write {}", path.display()))?;
    Ok((len, at + fresh.len() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REV: &str = "0123abcd";

    fn cache() -> (tempfile::TempDir, VectorCache) {
        let dir = tempfile::tempdir().unwrap();
        let c = VectorCache::at(dir.path().join("vectors"), 0).unwrap();
        (dir, c)
    }

    /// A hex text hash whose shard is `shard`, distinct per `n`.
    fn hash(shard: u8, n: u32) -> String {
        let mut k = *blake3::hash(&n.to_le_bytes()).as_bytes();
        k[0] = shard;
        k[1] &= 0x0f;
        hex_of(&k)
    }

    fn shard_file(c: &VectorCache, model: &str, dims: usize, shard: u8) -> PathBuf {
        c.root.join(Layout::new(model, REV, dims).dir_name()).join(format!("{shard:02x}0.rows"))
    }

    #[test]
    fn a_status_of_a_cache_not_made_yet_is_empty_and_makes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("vectors");
        let c = VectorCache::for_status(root.clone(), 1 << 20).unwrap();
        assert_eq!(c.status(), Status { models: vec![], other: 0 });
        assert!(!root.exists(), "a report does not create the directory it reports on");
    }

    #[test]
    fn a_row_put_comes_back_under_its_model_and_width_alone() {
        let (_d, c) = cache();
        let h = hash(7, 1);
        c.put("org/model", REV, 3, &[(&h, &[0.6, 0.0, 0.8])]).unwrap();
        assert_eq!(c.get("org/model", REV, 3, &[&h])[&h], vec![0.6, 0.0, 0.8]);
        assert_eq!(c.get("ORG/Model", REV, 3, &[&h]).len(), 1, "a hub id names one model in any case");
        assert!(c.get("org/model", REV, 4, &[&h]).is_empty(), "another width never crosses");
        assert!(c.get("org/other", REV, 3, &[&h]).is_empty(), "another model never crosses");
        assert!(c.get("org/model", REV, 3, &[&hash(7, 2)]).is_empty());
    }

    #[test]
    fn a_shard_under_another_models_directory_is_not_read_as_that_model() {
        let (_d, c) = cache();
        let h = hash(1, 1);
        c.put("a/b", REV, 2, &[(&h, &[1.0, 0.0])]).unwrap();
        c.put("a_b", REV, 2, &[(&hash(9, 9), &[1.0, 0.0])]).unwrap();
        assert_ne!(Layout::new("a/b", REV, 2).dir_name(), Layout::new("a_b", REV, 2).dir_name(), "ids that slug alike get their own directories");
        std::fs::copy(shard_file(&c, "a/b", 2, 1), shard_file(&c, "a_b", 2, 1)).unwrap();
        assert!(c.get("a_b", REV, 2, &[&h]).is_empty(), "the header names the model, and it is checked");
    }

    #[test]
    fn a_torn_tail_is_skipped_and_the_next_append_realigns() {
        let (_d, c) = cache();
        let (a, b) = (hash(3, 1), hash(3, 2));
        c.put("m", REV, 2, &[(&a, &[1.0, 0.0])]).unwrap();
        let shard = shard_file(&c, "m", 2, 3);
        std::fs::OpenOptions::new().append(true).open(&shard).unwrap().write_all(&[0xab; 11]).unwrap();
        assert_eq!(c.get("m", REV, 2, &[&a]).len(), 1, "the whole record before a torn one still reads");
        c.put("m", REV, 2, &[(&b, &[0.0, 1.0])]).unwrap();
        let got = c.get("m", REV, 2, &[&a, &b]);
        assert_eq!((got[&a].clone(), got[&b].clone()), (vec![1.0, 0.0], vec![0.0, 1.0]));
    }

    #[test]
    fn a_corrupt_record_is_dropped_alone_and_a_garbage_shard_is_no_panic() {
        let (_d, c) = cache();
        let (a, b) = (hash(5, 1), hash(5, 2));
        c.put("m", REV, 2, &[(&a, &[1.0, 0.0]), (&b, &[0.0, 1.0])]).unwrap();
        let shard = shard_file(&c, "m", 2, 5);
        let mut bytes = std::fs::read(&shard).unwrap();
        let first_row = Layout::new("m", REV, 2).header().len() + KEY + USED;
        bytes[first_row] ^= 0x40;
        std::fs::write(&shard, &bytes).unwrap();
        let got = c.get("m", REV, 2, &[&a, &b]);
        assert!(!got.contains_key(&a), "a record whose check fails is never returned");
        assert_eq!(got[&b], vec![0.0, 1.0]);
        for junk in [&b"junk"[..], &[], &MAGIC[..], &bytes[..30]] {
            std::fs::write(&shard, junk).unwrap();
            assert!(c.get("m", REV, 2, &[&a, &b]).is_empty());
        }
        c.put("m", REV, 2, &[(&a, &[1.0, 0.0])]).unwrap();
        assert_eq!(c.get("m", REV, 2, &[&a]).len(), 1, "a garbage shard is started over");
    }

    #[test]
    fn a_row_of_the_wrong_width_or_not_finite_is_never_stored() {
        let (_d, c) = cache();
        let (a, b, d) = (hash(2, 1), hash(2, 2), hash(2, 3));
        c.put("m", REV, 2, &[(&a, &[f32::NAN, 0.0]), (&b, &[1.0]), (&d, &[f32::INFINITY, 1.0]), ("not-hex", &[1.0, 1.0])]).unwrap();
        assert!(c.get("m", REV, 2, &[&a, &b, &d]).is_empty());
        assert!(!shard_file(&c, "m", 2, 2).exists(), "nothing was written for them");
    }

    #[test]
    fn writers_racing_on_one_shard_lose_and_tear_nothing() {
        let (_d, c) = cache();
        let threads: Vec<_> = (0..8u32).map(|t| {
            let c = c.clone();
            std::thread::spawn(move || {
                for i in 0..40u32 {
                    let h = hash(0, t * 1000 + i);
                    c.put("m", REV, 4, &[(&h, &[t as f32, i as f32, 1.0, 2.0])]).unwrap();
                }
            })
        }).collect();
        for t in threads { t.join().unwrap(); }
        let all: Vec<String> = (0..8u32).flat_map(|t| (0..40u32).map(move |i| hash(0, t * 1000 + i))).collect();
        let wanted: Vec<&str> = all.iter().map(String::as_str).collect();
        let got = c.get("m", REV, 4, &wanted);
        assert_eq!(got.len(), 320);
        assert_eq!(got[&hash(0, 3007)], vec![3.0, 7.0, 1.0, 2.0]);
    }

    #[test]
    fn prune_drops_the_least_recently_used_and_a_hit_counts_as_use() {
        let (_d, c) = cache();
        let (old, mid, new) = (hash(1, 1), hash(2, 2), hash(3, 3));
        c.put_at("m", REV, 2, &[(&old, &[1.0, 0.0])], 1_000).unwrap();
        c.put_at("m", REV, 2, &[(&mid, &[0.0, 1.0])], 2_000).unwrap();
        c.put_at("m", REV, 2, &[(&new, &[1.0, 1.0])], 3_000).unwrap();
        assert_eq!(c.get_at("m", REV, 2, &[&old], None, 4_000).len(), 1);
        let header = Layout::new("m", REV, 2).header().len() as u64;
        let rec = Layout::new("m", REV, 2).record_len() as u64;
        let pruned = c.prune(2 * rec + 3 * header).unwrap();
        assert_eq!((pruned.evicted, pruned.rows()), (1, 1));
        let got = c.get("m", REV, 2, &[&old, &mid, &new]);
        assert!(got.contains_key(&old) && got.contains_key(&new) && !got.contains_key(&mid), "{got:?}");
        assert_eq!(pruned.after, 2 * (header + rec));
    }

    #[test]
    fn rows_used_in_the_same_minute_are_trimmed_not_emptied() {
        let (_d, c) = cache();
        let rows: Vec<String> = (0..10u32).map(|i| hash(4, i)).collect();
        let pairs: Vec<(&str, &[f32])> = rows.iter().map(|h| (h.as_str(), &[1.0f32, 0.0][..])).collect();
        c.put_at("m", REV, 2, &pairs, 500).unwrap();
        let l = Layout::new("m", REV, 2);
        c.prune(4 * l.record_len() as u64 + l.header().len() as u64).unwrap();
        let wanted: Vec<&str> = rows.iter().map(String::as_str).collect();
        assert_eq!(c.get("m", REV, 2, &wanted).len(), 4);
    }

    #[test]
    fn a_write_over_the_cap_prunes_below_it() {
        let dir = tempfile::tempdir().unwrap();
        // Rows wide enough that every put crosses `WEIGH_EVERY`, so every put weighs the cache.
        let dims = (WEIGH_EVERY / 4) as usize;
        let l = Layout::new("m", REV, dims);
        let (rec, header) = (l.record_len() as u64, l.header().len() as u64);
        let c = VectorCache::at(dir.path().join("v"), 10 * (rec + header)).unwrap();
        let row = vec![0.5f32; dims];
        for i in 0..40u32 { c.put_at("m", REV, dims, &[(&hash(i as u8, i), &row)], i).unwrap(); }
        assert!(c.bytes() <= c.max(), "{} > {}", c.bytes(), c.max());
        assert_eq!(c.get("m", REV, dims, &[&hash(39, 39)]).len(), 1, "the newest row is kept");
        assert!(c.get("m", REV, dims, &[&hash(0, 0)]).is_empty(), "the oldest went first");
    }

    #[test]
    fn status_counts_rows_and_bytes_per_model() {
        let (_d, c) = cache();
        c.put("org/A", REV, 2, &[(&hash(1, 1), &[1.0, 0.0]), (&hash(2, 2), &[1.0, 0.0])]).unwrap();
        c.put("org/b", REV, 3, &[(&hash(1, 1), &[1.0, 0.0, 0.0])]).unwrap();
        let status = c.status();
        assert_eq!((status.other, status.bytes()), (0, c.bytes()));
        let mut s = status.models;
        s.sort_by(|a, b| a.model.cmp(&b.model));
        let l = Layout::new("org/a", REV, 2);
        assert_eq!(s[0], Usage { model: "org/a".into(), revision: REV.into(), dims: 2, rows: 2, bytes: 2 * (l.header().len() + l.record_len()) as u64, current: true });
        assert_eq!((s[1].model.as_str(), s[1].dims, s[1].rows), ("org/b", 3, 1));
    }

    #[test]
    fn another_revision_of_one_model_never_crosses() {
        let (_d, c) = cache();
        let h = hash(6, 1);
        c.put("org/model", "aaaa", 2, &[(&h, &[1.0, 0.0])]).unwrap();
        assert_eq!(c.get("org/model", "aaaa", 2, &[&h]).len(), 1);
        assert!(c.get("org/model", "bbbb", 2, &[&h]).is_empty(), "the hub's main moved: another model's vectors");
        assert_ne!(Layout::new("m", "aaaa", 2).dir_name(), Layout::new("m", "bbbb", 2).dir_name());
        assert!(Layout::new("m", "aaaa", 2).dir_name().ends_with(&format!(".v{VERSION}")), "the format names its directories");
    }

    #[test]
    fn a_bounded_lookup_reads_its_fullest_shards_first() {
        let (_d, c) = cache();
        // Shard 3 holds two of the wanted rows, shards 1, 2 and 4 one each.
        let rows: Vec<String> = [(1, 0), (2, 1), (3, 2), (3, 3), (4, 4)].iter().map(|&(s, n)| hash(s, n)).collect();
        let pairs: Vec<(&str, &[f32])> = rows.iter().map(|h| (h.as_str(), &[1.0f32, 0.0][..])).collect();
        c.put("m", REV, 2, &pairs).unwrap();
        let wanted: Vec<&str> = rows.iter().map(String::as_str).collect();
        let found = c.get_within("m", REV, 2, &wanted, 2);
        let mut got: Vec<&str> = found.keys().map(String::as_str).collect();
        got.sort();
        let mut first = vec![rows[2].as_str(), rows[3].as_str(), rows[0].as_str()];
        first.sort();
        assert_eq!(got, first, "the shard holding the most wanted rows is read first, then the lowest named");
        assert_eq!(c.get_within("m", REV, 2, &wanted, 4), c.get("m", REV, 2, &wanted));
        assert_eq!(c.get_within("m", REV, 2, &wanted, 4).len(), 5);
    }

    #[test]
    fn a_key_already_held_is_never_stored_again_and_a_repeat_is_pruned_as_one() {
        let (_d, c) = cache();
        let h = hash(8, 1);
        c.put("m", REV, 2, &[(&h, &[1.0, 0.0]), (&h, &[1.0, 0.0])]).unwrap();
        let l = Layout::new("m", REV, 2);
        assert_eq!(c.bytes(), (l.header().len() + l.record_len()) as u64, "one record for one key");
        c.put("m", REV, 2, &[(&h, &[1.0, 0.0])]).unwrap();
        assert_eq!(c.bytes(), (l.header().len() + l.record_len()) as u64, "a later put of a held key writes nothing");
        // A repeat from a release that did not check what the shard held.
        let path = shard_file(&c, "m", 2, 8);
        let mut bytes = std::fs::read(&path).unwrap();
        let rec = bytes[l.header().len()..].to_vec();
        bytes.extend(rec);
        std::fs::write(&path, bytes).unwrap();
        let p = c.prune(u64::MAX).unwrap();
        assert_eq!((p.evicted, p.repeated, p.older), (0, 1, 0), "under the cap only the repeat goes");
        assert_eq!(c.get("m", REV, 2, &[&h]).len(), 1);
    }

    #[test]
    fn a_key_whose_held_record_is_corrupt_is_stored_again_and_read() {
        let (_d, c) = cache();
        let h = hash(8, 1);
        c.put("m", REV, 2, &[(&h, &[1.0, 0.0])]).unwrap();
        let path = shard_file(&c, "m", 2, 8);
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(&path, bytes).unwrap();
        assert!(c.get("m", REV, 2, &[&h]).is_empty(), "a record that fails its check is never returned");
        c.put("m", REV, 2, &[(&h, &[0.0, 1.0])]).unwrap();
        assert_eq!(c.get("m", REV, 2, &[&h])[&h], vec![0.0, 1.0], "the record appended after it is");
    }

    /// A shard as format 1 wrote it: no revision in the header, in a directory without a suffix.
    fn format_one(c: &VectorCache, model: &str, dims: usize, rows: usize) -> PathBuf {
        let dir = c.root.join(format!("{model}-00000000@{dims}"));
        std::fs::create_dir_all(&dir).unwrap();
        let mut bytes = MAGIC.to_vec();
        for w in [1u32, dims as u32, model.len() as u32] { bytes.extend_from_slice(&w.to_le_bytes()); }
        bytes.extend_from_slice(model.as_bytes());
        let l = Layout::new(model, "", dims);
        for i in 0..rows { bytes.extend(l.record(&key_of(&hash(1, i as u32)).unwrap(), &vec![1.0; dims], 0)); }
        let path = dir.join("010.rows");
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn an_earlier_format_is_listed_then_pruned_and_a_later_one_is_left_alone() {
        let (_d, c) = cache();
        let old = format_one(&c, "m", 2, 3);
        let later = c.root.join(Layout::new("m", REV, 2).dir_name());
        std::fs::create_dir_all(&later).unwrap();
        let mut newer = MAGIC.to_vec();
        newer.extend_from_slice(&(VERSION + 1).to_le_bytes());
        newer.extend_from_slice(&[7; 40]);
        std::fs::write(later.join("020.rows"), &newer).unwrap();
        std::fs::write(later.join("030.rows"), b"not a shard").unwrap();
        let status = c.status();
        assert_eq!(status.other, (newer.len() + 11) as u64, "a later format and garbage are weighed apart");
        assert_eq!(status.bytes(), c.bytes(), "and with the rest against the cap");
        let s = status.models;
        assert_eq!(s.len(), 1, "a later format is not this build's to describe: {s:?}");
        assert_eq!((s[0].rows, s[0].current), (3, false));
        c.put("m", REV, 2, &[(&hash(2, 9), &[1.0, 0.0])]).unwrap();
        assert_eq!(std::fs::read(later.join("020.rows")).unwrap(), newer, "an append does not start a later format over");
        let p = c.prune(u64::MAX).unwrap();
        assert_eq!((p.older, p.rows()), (3, 3));
        assert!(!old.exists());
        assert_eq!(std::fs::read(later.join("020.rows")).unwrap(), newer, "nor does a prune remove it");
        assert!(!later.join("030.rows").exists(), "garbage is removed");
    }

    #[cfg(unix)]
    #[test]
    fn a_shard_that_cannot_be_rewritten_is_counted_and_the_prune_goes_on() {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: geteuid has no preconditions and cannot fail.
        if unsafe { libc::geteuid() } == 0 { return; }
        let (_d, c) = cache();
        c.put("a", REV, 2, &[(&hash(1, 1), &[1.0, 0.0])]).unwrap();
        c.put("b", REV, 2, &[(&hash(1, 2), &[1.0, 0.0])]).unwrap();
        let held = c.root.join(Layout::new("a", REV, 2).dir_name());
        std::fs::set_permissions(&held, std::fs::Permissions::from_mode(0o500)).unwrap();
        let p = c.prune(0);
        std::fs::set_permissions(&held, std::fs::Permissions::from_mode(0o700)).unwrap();
        let p = p.unwrap();
        assert_eq!((p.failed, p.evicted), (1, 1), "{p:?}");
        assert!(c.get("b", REV, 2, &[&hash(1, 2)]).is_empty(), "the shard after the failure was still pruned");
    }

    #[test]
    fn a_background_prune_starts_again_once_its_interval_has_passed() {
        let mut last = None;
        let t0 = std::time::Instant::now();
        assert!(due(&mut last, t0));
        assert!(!due(&mut last, t0 + PRUNE_EVERY / 2), "not again within the interval");
        assert!(due(&mut last, t0 + PRUNE_EVERY), "a long-lived reader keeps its cap");
    }

    #[cfg(unix)]
    #[test]
    fn the_root_is_made_private_and_refused_when_it_is_not() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("cache/zerograph/vectors");
        VectorCache::at(root.clone(), 0).unwrap();
        assert_eq!(std::fs::metadata(&root).unwrap().permissions().mode() & 0o777, 0o700);
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(VectorCache::at(root.clone(), 0).is_err(), "a group- or world-readable root is not ours alone");
        std::fs::remove_dir(&root).unwrap();
        let elsewhere = home.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &root).unwrap();
        assert!(VectorCache::at(root, 0).is_err(), "a link in the root's place is not a directory of ours");
    }

    #[test]
    fn a_relative_or_unconfined_override_is_refused_not_replaced() {
        let abs = std::env::temp_dir().join("v");
        let ok = |named: Option<&std::path::Path>, platform: Option<&std::path::Path>, within: Option<&std::path::Path>| {
            root_from(named.map(|p| p.as_os_str().to_owned()), platform.map(PathBuf::from), within).ok()
        };
        assert_eq!(ok(Some("relative/dir".as_ref()), Some(&abs), None), None, "refused, not the platform root");
        assert_eq!(ok(Some(&abs), None, None), Some(abs.clone()));
        assert_eq!(ok(Some("".as_ref()), Some(&abs), None), Some(abs.clone()), "an empty variable is unset");
        assert_eq!(ok(None, Some("rel".as_ref()), None), None);
        assert!(format!("{:#}", root_from(None, None, None).unwrap_err()).contains("home directory"));
        let base = abs.parent().unwrap();
        assert_eq!(ok(Some(&base.join("zg")), None, Some(base)), Some(base.join("zg")));
        let outside = std::env::temp_dir().join("elsewhere");
        assert_eq!(ok(Some(&outside), Some(&abs), Some(&abs)), None, "outside the confining base");
        assert_eq!(ok(Some(&abs.join("..").join("..").join("x")), None, Some(&abs)), None, "`..` cannot climb out");
    }
}
