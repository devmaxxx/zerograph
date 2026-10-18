//! Vectors this user embedded on this machine, keyed by what a vector is a function of: the
//! model, its width and the row's text. A cold build is almost all embedding, and a second
//! checkout of the same code — a worktree, a clone, a fork — embeds the same texts again, so a row
//! any of them embedded once is a read for the rest.
//!
//! One directory per model and width, 4096 append-only shard files in each, picked by the first
//! twelve bits of the row's hash. A reader reads every shard a wanted row falls in whole, so the
//! count keeps a shard near a megabyte at the default cap: an edit owing a few rows reads a few
//! megabytes, not a sixteenth of the cache. A shard is a header naming its model and width, then fixed-width
//! records, so the stride is the index and there is no second file to tear. Readers take no
//! lock: they count whole records and check each one, so a record mid-append, cut short by a
//! crash, or corrupted on disk is skipped, never returned. Writers append under a lock of the
//! shard's own, held for one write.
//!
//! Only `DenseIndex::sync_chunked`'s embed loop writes here. A store's `vectors.f32` is never
//! imported, so rows a branch committed under `.zerograph/` cannot seed it.

use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"ZGVCACHE";
const VERSION: u32 = 1;
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
pub struct Usage { pub model: String, pub dims: usize, pub rows: u64, pub bytes: u64 }

#[derive(Debug, Default, PartialEq)]
pub struct Pruned { pub rows: u64, pub before: u64, pub after: u64 }

/// `$ZEROGRAPH_VECTOR_CACHE`, else `%LOCALAPPDATA%\zerograph\vectors` on Windows and
/// `~/.cache/zerograph/vectors` elsewhere. A relative override is refused: it would put the cache
/// wherever each process happens to stand.
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
        root_from(crate::env_os("VECTOR_CACHE"), platform, None)
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

/// What a shard of one model at one width looks like. A hub id names one model in any case, as
/// `DenseIndex::written_by` has it, so the id is held lower-cased.
struct Layout { model: String, dims: usize }

impl Layout {
    fn new(model: &str, dims: usize) -> Layout { Layout { model: model.to_ascii_lowercase(), dims } }

    fn header(&self) -> Vec<u8> {
        let mut h = Vec::with_capacity(20 + self.model.len());
        h.extend_from_slice(MAGIC);
        h.extend_from_slice(&VERSION.to_le_bytes());
        h.extend_from_slice(&(self.dims as u32).to_le_bytes());
        h.extend_from_slice(&(self.model.len() as u32).to_le_bytes());
        h.extend_from_slice(self.model.as_bytes());
        h
    }

    fn row_len(&self) -> usize { self.dims * 4 }
    fn record_len(&self) -> usize { KEY + USED + self.row_len() + CHECK }

    /// The directory name: readable, and unique per model by the hash, so two ids that slug alike
    /// never share shards. The header is checked on every read all the same.
    fn dir_name(&self) -> String {
        let slug: String = self.model.chars().take(64)
            .map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect();
        let tag = &blake3::hash(self.model.as_bytes()).to_hex()[..8];
        format!("{slug}-{tag}@{}", self.dims)
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

/// The model and width a shard's header names, for `status`, which has no layout to compare with.
fn read_header(bytes: &[u8]) -> Option<Layout> {
    if !bytes.starts_with(MAGIC) || bytes.len() < 20 { return None; }
    let word = |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    if word(8) != VERSION { return None; }
    let (dims, len) = (word(12) as usize, word(16) as usize);
    let model = std::str::from_utf8(bytes.get(20..20 + len)?).ok()?;
    (dims > 0).then(|| Layout::new(model, dims))
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

    /// What a prune trims to: under the cap by enough that the next few writes do not prune again,
    /// and to nothing when the cap is 0.
    pub fn prune_target(&self) -> u64 { self.max / 100 * PRUNE_TO_PERCENT }

    pub fn root(&self) -> &Path { &self.root }
    pub fn max(&self) -> u64 { self.max }

    /// Rows for as many of the `wanted` hex text hashes as the cache holds under `model` at `dims`.
    pub fn get(&self, model: &str, dims: usize, wanted: &[&str]) -> HashMap<String, Vec<f32>> {
        self.get_at(model, dims, wanted, now_minutes())
    }

    fn get_at(&self, model: &str, dims: usize, wanted: &[&str], now: u32) -> HashMap<String, Vec<f32>> {
        let mut found = HashMap::new();
        if dims == 0 || model.is_empty() { return found; }
        let layout = Layout::new(model, dims);
        let dir = self.root.join(layout.dir_name());
        let mut by_shard: HashMap<String, HashSet<[u8; KEY]>> = HashMap::new();
        for k in wanted.iter().filter_map(|h| key_of(h)) { by_shard.entry(shard_of(&k)).or_default().insert(k); }
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

    /// Appends `rows` — hex text hash and vector — under `model` at `dims`, then prunes, or starts
    /// a prune, when the cache is over its cap. A row of another width or with a non-finite float is not stored.
    pub fn put(&self, model: &str, dims: usize, rows: &[(&str, &[f32])]) -> Result<()> {
        self.put_at(model, dims, rows, now_minutes())
    }

    fn put_at(&self, model: &str, dims: usize, rows: &[(&str, &[f32])], now: u32) -> Result<()> {
        if dims == 0 || model.is_empty() { return Ok(()); }
        let layout = Layout::new(model, dims);
        let mut by_shard: HashMap<String, Vec<u8>> = HashMap::new();
        for (hash, row) in rows {
            let Some(key) = key_of(hash) else { continue };
            if row.len() != dims || !row.iter().all(|x| x.is_finite()) { continue; }
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
                true => prune_detached(),
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

    /// Rows a reader would return and bytes on disk, per model. Every shard is read whole, so a
    /// torn, corrupt or repeated record is not counted as a row; its bytes still are.
    pub fn status(&self) -> Vec<Usage> {
        let mut out = Vec::new();
        for dir in self.model_dirs() {
            let mut usage: Option<Usage> = None;
            for shard in Self::shards(&dir) {
                let Ok(bytes) = open(&shard, false, false).and_then(|mut f| read_all(&mut f)) else { continue };
                let Some(layout) = read_header(&bytes) else { continue };
                let mut keys = HashSet::new();
                let rows = layout.records(&bytes)
                    .filter(|(_, rec)| layout.row_of(rec).is_some() && keys.insert(&rec[..KEY]))
                    .count() as u64;
                let u = usage.get_or_insert_with(|| Usage { model: layout.model.clone(), dims: layout.dims, rows: 0, bytes: 0 });
                u.rows += rows;
                u.bytes += bytes.len() as u64;
            }
            out.extend(usage);
        }
        out
    }

    /// Trims the cache to `target` bytes, least recently used first, and drops every torn,
    /// corrupt or repeated record on the way. A prune already running elsewhere is left to it.
    pub fn prune(&self, target: u64) -> Result<Pruned> {
        let lock_path = self.root.join("prune.lock");
        let lock = open(&lock_path, true, true).with_context(|| format!("open {}", lock_path.display()))?;
        if lock.try_lock().is_err() { return Ok(Pruned::default()); }
        let before = self.bytes();
        // The cut is found over every model at once: recency is the only order, whoever wrote it.
        let mut uses: Vec<(u32, u64)> = Vec::new();
        let mut headers = 0u64;
        for dir in self.model_dirs() {
            for shard in Self::shards(&dir) {
                let Ok(bytes) = open(&shard, false, false).and_then(|mut f| read_all(&mut f)) else { continue };
                let Some(layout) = read_header(&bytes) else { continue };
                headers += layout.header().len() as u64;
                let rl = layout.record_len() as u64;
                uses.extend(layout.records(&bytes).filter(|(_, r)| layout.row_of(r).is_some()).map(|(_, r)| (Layout::used_of(r), rl)));
            }
        }
        let cut = Cut::new(uses, target.saturating_sub(headers));
        let mut dropped = 0;
        let mut ties = cut.ties;
        for dir in self.model_dirs() {
            for shard in Self::shards(&dir) {
                let Some(name) = shard.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
                let _guard = lock_shard(&dir, &name)?;
                let Ok(bytes) = open(&shard, false, false).and_then(|mut f| read_all(&mut f)) else { continue };
                let Some(layout) = read_header(&bytes) else {
                    std::fs::remove_file(&shard).with_context(|| format!("remove {}", shard.display()))?;
                    continue;
                };
                let header = layout.header();
                let mut kept = header.clone();
                let mut seen = HashSet::new();
                let mut whole = 0u64;
                for (_, rec) in layout.records(&bytes) {
                    whole += 1;
                    if layout.row_of(rec).is_none() || !seen.insert(rec[..KEY].to_vec()) { continue; }
                    let used = Layout::used_of(rec);
                    let keep = match cut.at {
                        None => true,
                        Some(at) if used > at => true,
                        Some(at) if used == at && ties >= rec.len() as u64 => { ties -= rec.len() as u64; true }
                        Some(_) => false,
                    };
                    if keep { kept.extend_from_slice(rec); }
                }
                let kept_rows = ((kept.len() - header.len()) / layout.record_len()) as u64;
                dropped += whole - kept_rows;
                if kept.len() == bytes.len() { continue; }
                if kept_rows == 0 {
                    std::fs::remove_file(&shard).with_context(|| format!("remove {}", shard.display()))?;
                    continue;
                }
                let tmp = dir.join(format!("{name}.rows.tmp"));
                let _ = std::fs::remove_file(&tmp);
                let mut f = std::fs::OpenOptions::new();
                f.write(true).create_new(true);
                let mut f = crate::store::open_unlinked(&mut f, &tmp).with_context(|| format!("create {}", tmp.display()))?;
                f.write_all(&kept).with_context(|| format!("write {}", tmp.display()))?;
                drop(f);
                std::fs::rename(&tmp, &shard).with_context(|| format!("replace {}", shard.display()))?;
            }
        }
        drop(lock);
        Ok(Pruned { rows: dropped, before, after: self.bytes() })
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

/// A model's directory under the root, made private and never a link.
/// `zerograph cache prune` in a background process, once per process: a resident reader over the
/// cap would otherwise start one on every write until the first had finished.
fn prune_detached() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) { return; }
    if let Err(err) = crate::refresh::spawn_quiet(&["cache", "prune"]) {
        warn_once("over its cap and not pruned", &err);
    }
}

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

/// `records` appended to one shard under its lock. A shard that does not start with this layout's
/// header — new, garbage, or another format — is started over; a torn tail a crash left is cut
/// back to the last whole record first, or every record after it would be read out of step.
/// Returns the shard's length before and after.
fn append(dir: &Path, shard: &str, layout: &Layout, records: &[u8]) -> Result<(u64, u64)> {
    let _guard = lock_shard(dir, shard)?;
    let path = dir.join(format!("{shard}.rows"));
    let mut f = open(&path, true, true).with_context(|| format!("open {}", path.display()))?;
    let header = layout.header();
    let len = f.metadata().with_context(|| format!("stat {}", path.display()))?.len();
    let mut head = vec![0u8; header.len()];
    let fits = len >= header.len() as u64 && f.read_exact(&mut head).is_ok() && head == header;
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
    write_at(&f, at, records).with_context(|| format!("write {}", path.display()))?;
    Ok((len, at + records.len() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        c.root.join(Layout::new(model, dims).dir_name()).join(format!("{shard:02x}0.rows"))
    }

    #[test]
    fn a_row_put_comes_back_under_its_model_and_width_alone() {
        let (_d, c) = cache();
        let h = hash(7, 1);
        c.put("org/model", 3, &[(&h, &[0.6, 0.0, 0.8])]).unwrap();
        assert_eq!(c.get("org/model", 3, &[&h])[&h], vec![0.6, 0.0, 0.8]);
        assert_eq!(c.get("ORG/Model", 3, &[&h]).len(), 1, "a hub id names one model in any case");
        assert!(c.get("org/model", 4, &[&h]).is_empty(), "another width never crosses");
        assert!(c.get("org/other", 3, &[&h]).is_empty(), "another model never crosses");
        assert!(c.get("org/model", 3, &[&hash(7, 2)]).is_empty());
    }

    #[test]
    fn a_shard_under_another_models_directory_is_not_read_as_that_model() {
        let (_d, c) = cache();
        let h = hash(1, 1);
        c.put("a/b", 2, &[(&h, &[1.0, 0.0])]).unwrap();
        c.put("a_b", 2, &[(&hash(9, 9), &[1.0, 0.0])]).unwrap();
        assert_ne!(Layout::new("a/b", 2).dir_name(), Layout::new("a_b", 2).dir_name(), "ids that slug alike get their own directories");
        std::fs::copy(shard_file(&c, "a/b", 2, 1), shard_file(&c, "a_b", 2, 1)).unwrap();
        assert!(c.get("a_b", 2, &[&h]).is_empty(), "the header names the model, and it is checked");
    }

    #[test]
    fn a_torn_tail_is_skipped_and_the_next_append_realigns() {
        let (_d, c) = cache();
        let (a, b) = (hash(3, 1), hash(3, 2));
        c.put("m", 2, &[(&a, &[1.0, 0.0])]).unwrap();
        let shard = shard_file(&c, "m", 2, 3);
        std::fs::OpenOptions::new().append(true).open(&shard).unwrap().write_all(&[0xab; 11]).unwrap();
        assert_eq!(c.get("m", 2, &[&a]).len(), 1, "the whole record before a torn one still reads");
        c.put("m", 2, &[(&b, &[0.0, 1.0])]).unwrap();
        let got = c.get("m", 2, &[&a, &b]);
        assert_eq!((got[&a].clone(), got[&b].clone()), (vec![1.0, 0.0], vec![0.0, 1.0]));
    }

    #[test]
    fn a_corrupt_record_is_dropped_alone_and_a_garbage_shard_is_no_panic() {
        let (_d, c) = cache();
        let (a, b) = (hash(5, 1), hash(5, 2));
        c.put("m", 2, &[(&a, &[1.0, 0.0]), (&b, &[0.0, 1.0])]).unwrap();
        let shard = shard_file(&c, "m", 2, 5);
        let mut bytes = std::fs::read(&shard).unwrap();
        let first_row = Layout::new("m", 2).header().len() + KEY + USED;
        bytes[first_row] ^= 0x40;
        std::fs::write(&shard, &bytes).unwrap();
        let got = c.get("m", 2, &[&a, &b]);
        assert!(!got.contains_key(&a), "a record whose check fails is never returned");
        assert_eq!(got[&b], vec![0.0, 1.0]);
        for junk in [&b"junk"[..], &[], &MAGIC[..], &bytes[..30]] {
            std::fs::write(&shard, junk).unwrap();
            assert!(c.get("m", 2, &[&a, &b]).is_empty());
        }
        c.put("m", 2, &[(&a, &[1.0, 0.0])]).unwrap();
        assert_eq!(c.get("m", 2, &[&a]).len(), 1, "a garbage shard is started over");
    }

    #[test]
    fn a_row_of_the_wrong_width_or_not_finite_is_never_stored() {
        let (_d, c) = cache();
        let (a, b, d) = (hash(2, 1), hash(2, 2), hash(2, 3));
        c.put("m", 2, &[(&a, &[f32::NAN, 0.0]), (&b, &[1.0]), (&d, &[f32::INFINITY, 1.0]), ("not-hex", &[1.0, 1.0])]).unwrap();
        assert!(c.get("m", 2, &[&a, &b, &d]).is_empty());
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
                    c.put("m", 4, &[(&h, &[t as f32, i as f32, 1.0, 2.0])]).unwrap();
                }
            })
        }).collect();
        for t in threads { t.join().unwrap(); }
        let all: Vec<String> = (0..8u32).flat_map(|t| (0..40u32).map(move |i| hash(0, t * 1000 + i))).collect();
        let wanted: Vec<&str> = all.iter().map(String::as_str).collect();
        let got = c.get("m", 4, &wanted);
        assert_eq!(got.len(), 320);
        assert_eq!(got[&hash(0, 3007)], vec![3.0, 7.0, 1.0, 2.0]);
    }

    #[test]
    fn prune_drops_the_least_recently_used_and_a_hit_counts_as_use() {
        let (_d, c) = cache();
        let (old, mid, new) = (hash(1, 1), hash(2, 2), hash(3, 3));
        c.put_at("m", 2, &[(&old, &[1.0, 0.0])], 1_000).unwrap();
        c.put_at("m", 2, &[(&mid, &[0.0, 1.0])], 2_000).unwrap();
        c.put_at("m", 2, &[(&new, &[1.0, 1.0])], 3_000).unwrap();
        assert_eq!(c.get_at("m", 2, &[&old], 4_000).len(), 1);
        let header = Layout::new("m", 2).header().len() as u64;
        let rec = Layout::new("m", 2).record_len() as u64;
        let pruned = c.prune(2 * rec + 3 * header).unwrap();
        assert_eq!(pruned.rows, 1);
        let got = c.get("m", 2, &[&old, &mid, &new]);
        assert!(got.contains_key(&old) && got.contains_key(&new) && !got.contains_key(&mid), "{got:?}");
        assert_eq!(pruned.after, 2 * (header + rec));
    }

    #[test]
    fn rows_used_in_the_same_minute_are_trimmed_not_emptied() {
        let (_d, c) = cache();
        let rows: Vec<String> = (0..10u32).map(|i| hash(4, i)).collect();
        let pairs: Vec<(&str, &[f32])> = rows.iter().map(|h| (h.as_str(), &[1.0f32, 0.0][..])).collect();
        c.put_at("m", 2, &pairs, 500).unwrap();
        let l = Layout::new("m", 2);
        c.prune(4 * l.record_len() as u64 + l.header().len() as u64).unwrap();
        let wanted: Vec<&str> = rows.iter().map(String::as_str).collect();
        assert_eq!(c.get("m", 2, &wanted).len(), 4);
    }

    #[test]
    fn a_write_over_the_cap_prunes_below_it() {
        let dir = tempfile::tempdir().unwrap();
        // Rows wide enough that every put crosses `WEIGH_EVERY`, so every put weighs the cache.
        let dims = (WEIGH_EVERY / 4) as usize;
        let l = Layout::new("m", dims);
        let (rec, header) = (l.record_len() as u64, l.header().len() as u64);
        let c = VectorCache::at(dir.path().join("v"), 10 * (rec + header)).unwrap();
        let row = vec![0.5f32; dims];
        for i in 0..40u32 { c.put_at("m", dims, &[(&hash(i as u8, i), &row)], i).unwrap(); }
        assert!(c.bytes() <= c.max(), "{} > {}", c.bytes(), c.max());
        assert_eq!(c.get("m", dims, &[&hash(39, 39)]).len(), 1, "the newest row is kept");
        assert!(c.get("m", dims, &[&hash(0, 0)]).is_empty(), "the oldest went first");
    }

    #[test]
    fn status_counts_rows_and_bytes_per_model() {
        let (_d, c) = cache();
        c.put("org/A", 2, &[(&hash(1, 1), &[1.0, 0.0]), (&hash(2, 2), &[1.0, 0.0])]).unwrap();
        c.put("org/b", 3, &[(&hash(1, 1), &[1.0, 0.0, 0.0])]).unwrap();
        let mut s = c.status();
        s.sort_by(|a, b| a.model.cmp(&b.model));
        let l = Layout::new("org/a", 2);
        assert_eq!(s[0], Usage { model: "org/a".into(), dims: 2, rows: 2, bytes: 2 * (l.header().len() + l.record_len()) as u64 });
        assert_eq!((s[1].model.as_str(), s[1].dims, s[1].rows), ("org/b", 3, 1));
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
