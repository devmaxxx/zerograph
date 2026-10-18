use crate::model::Graph;
use crate::walk::Manifest;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub struct Store { repo: PathBuf }

/// The store's directory, and the name it had before the rename. A store under the old name is
/// read where it stands and moved by the first write, so an upgrade neither rebuilds nor re-embeds.
pub(crate) const DIR: &str = ".zerograph";
pub(crate) const LEGACY_DIR: &str = ".repograph";

fn present(p: &Path) -> bool { std::fs::symlink_metadata(p).is_ok() }

/// Where the store is right now: `.zerograph` whenever anything stands at that name, else a
/// `.repograph` no write has moved yet, else `.zerograph` for a store not built. Resolved on each
/// call and never kept, so a server or an MCP process that outlives the move follows it.
pub(crate) fn dir_of(repo: &Path) -> PathBuf {
    let dir = repo.join(DIR);
    if present(&dir) { return dir; }
    let legacy = repo.join(LEGACY_DIR);
    if present(&legacy) { legacy } else { dir }
}

/// [`dir_of`]'s name, for a line that tells the reader which directory to open: a store the move
/// could not reach stays under the old name, and a line that said `.zerograph` would send them to
/// a directory that is not there.
pub(crate) fn dir_name_of(repo: &Path) -> &'static str {
    if dir_of(repo).ends_with(LEGACY_DIR) { LEGACY_DIR } else { DIR }
}

/// Where a load got its value. The JSON is what a store is defined by and what any other
/// binary reads; the mirror is a postcard copy of the same value written beside it, because
/// parsing 10 MB of graph JSON costs every `ask` 18 ms and decoding the same graph a few.
/// `Json` carries the stamp of the bytes that were parsed, so a mirror written from that value
/// later is labelled with them even if another writer has renamed a new graph in since.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source { Absent, Json(MirrorStamp), Mirror }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MirrorStamp(Option<(u128, u64)>);

/// A `Graph` that gained a field is a new mirror shape, and a magic that says so is one fewer
/// thing that depends on postcard failing at the right byte: `RGM1` mirrors hold the struct from
/// before `pending`, and are passed over rather than decoded into it.
const MIRROR_MAGIC: &[u8; 4] = b"RGM2";
const MIRROR_HEADER: usize = 4 + 12 + 16 + 8;

fn stamp(p: &Path) -> Option<(u128, u64)> { stamp_of(&std::fs::metadata(p).ok()?) }

fn stamp_of(m: &std::fs::Metadata) -> Option<(u128, u64)> {
    let mtime = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some((mtime, m.len()))
}

/// What a mirror was written from. It is trusted only while the JSON's stat stamp and the
/// release that wrote it both still match: postcard is not self-describing, so a struct that
/// changed shape between releases would otherwise decode into a wrong graph without one error.
fn mirror_header(stamp: (u128, u64)) -> [u8; MIRROR_HEADER] {
    let mut h = [0u8; MIRROR_HEADER];
    h[..4].copy_from_slice(MIRROR_MAGIC);
    let v = env!("CARGO_PKG_VERSION").as_bytes();
    let n = v.len().min(12);
    h[4..4 + n].copy_from_slice(&v[..n]);
    h[16..32].copy_from_slice(&stamp.0.to_le_bytes());
    h[32..40].copy_from_slice(&stamp.1.to_le_bytes());
    h
}

fn mirror_name(json: &str) -> String { format!("{}.bin", json.trim_end_matches(".json")) }

#[cfg(unix)]
pub(crate) fn rename_over(from: &Path, to: &Path) -> std::io::Result<()> { std::fs::rename(from, to) }

/// A rename over a file some other program has open without delete sharing — an indexer, a sync
/// client, a scanner — is refused for as long as that handle lives, which is milliseconds. A held
/// *destination* is reported as access-denied rather than as a sharing violation (measured on a
/// Windows runner, where the sharing-violation-only form of this let both tests fail with os error
/// 5); std's own retry for access-denied is a second rename with POSIX semantics, which the
/// holder's share mode refuses just as flatly, so waiting is the only thing left. Both errors are
/// retried, for about half a second in all, before the store is left as it was.
#[cfg(windows)]
pub(crate) fn rename_over(from: &Path, to: &Path) -> std::io::Result<()> {
    const HELD_BY_ANOTHER_PROCESS: [i32; 2] = [5, 32];
    let mut wait = std::time::Duration::from_millis(1);
    loop {
        match std::fs::rename(from, to) {
            Err(e) if e.raw_os_error().is_some_and(|c| HELD_BY_ANOTHER_PROCESS.contains(&c))
                && wait < std::time::Duration::from_millis(512) => {
                std::thread::sleep(wait);
                wait *= 2;
            }
            r => return r,
        }
    }
}

fn is_link(p: &Path) -> bool { std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()) }

/// Opens a store file, never through a symlink. A cloned repository controls every entry under
/// `.zerograph/`, so a committed link there would carry this process's write — a log line, a
/// vector row, a truncation — to a file outside the repository. The check names the refusal; the
/// open flag closes the gap between the check and the open.
pub(crate) fn open_unlinked(o: &mut std::fs::OpenOptions, p: &Path) -> std::io::Result<std::fs::File> {
    if is_link(p) { return Err(std::io::Error::other("refusing to write through a symlink")); }
    open_nofollow(o, p)
}

/// Makes `dir` and refuses it unless it is ours alone: another user may have created the name
/// first, or planted a symlink at it, and what this process then puts there — a socket, cached
/// vectors — is theirs to read and replace. Only the leaf is made private: the directories above
/// it are the user's own to arrange.
#[cfg(unix)]
pub(crate) fn prepare_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    if let Some(above) = dir.parent() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(above)
            .with_context(|| format!("create {}", above.display()))?;
    }
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e).with_context(|| format!("create {}", dir.display())),
    }
    let meta = std::fs::symlink_metadata(dir).with_context(|| format!("stat {}", dir.display()))?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    let mine = meta.uid() == unsafe { libc::geteuid() };
    if !meta.is_dir() || !mine || meta.permissions().mode() & 0o077 != 0 {
        anyhow::bail!("{} is not a directory private to this user", dir.display());
    }
    Ok(())
}

/// `dir` with its parent's links resolved, or a refusal when another user could move it: an
/// ancestor another user owns, or one of root's that others may write without the sticky bit,
/// lets them rename the path away and put their own directory in its place after
/// `prepare_private_dir` has checked it. This user's own ancestors are theirs whatever the mode,
/// which a umask of 0002 leaves group-writable. Both the ancestors as spelled and those the links
/// lead to are asked about, since either is a place the path can be moved from. Ancestors that do
/// not exist yet are made by this user. `dir` itself is left unresolved, so that
/// `prepare_private_dir` still refuses it when it is a link.
#[cfg(unix)]
pub(crate) fn refuse_shared_ancestors(dir: &Path) -> Result<PathBuf> {
    let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else { return Ok(dir.to_path_buf()) };
    let real = resolve_existing(parent).join(name);
    for spelled in [dir, real.as_path()] {
        for above in spelled.ancestors().skip(1) {
            if let Some(reason) = shared_with_others(above) {
                anyhow::bail!("{} sits under {}, which {reason}", dir.display(), above.display());
            }
        }
    }
    Ok(real)
}

/// `path` with the part of it that exists canonicalized and the rest appended as spelled.
#[cfg(unix)]
fn resolve_existing(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut at = path;
    loop {
        if let Ok(real) = at.canonicalize() {
            return rest.iter().rev().fold(real, |p, part| p.join(part));
        }
        match (at.parent(), at.file_name()) {
            (Some(up), Some(part)) => { rest.push(part.to_os_string()); at = up; }
            _ => return path.to_path_buf(),
        }
    }
}

#[cfg(unix)]
fn shared_with_others(dir: &Path) -> Option<&'static str> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let meta = std::fs::metadata(dir).ok()?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    others_may_move(meta.uid(), unsafe { libc::geteuid() }, meta.permissions().mode())
}

#[cfg(unix)]
fn others_may_move(owner: u32, me: u32, mode: u32) -> Option<&'static str> {
    if owner == me { return None; }
    // Another user may always rename what is in a directory they own, whatever its mode.
    if owner != 0 { return Some("another user owns"); }
    (mode & 0o022 != 0 && mode & 0o1000 == 0).then_some("others can write without the sticky bit")
}

/// The profile directory's own ACL is what keeps others out on Windows; all that is owed here is
/// a directory, and never one reached through a reparse point.
#[cfg(not(unix))]
pub(crate) fn prepare_private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    if is_link(dir) { anyhow::bail!("{} is a link, not a directory of ours", dir.display()); }
    Ok(())
}

/// Apart from the check so that the flag can be tested on its own: with the check in front, a
/// planted link never reaches it.
fn open_nofollow(o: &mut std::fs::OpenOptions, p: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    { use std::os::unix::fs::OpenOptionsExt; o.custom_flags(libc::O_NOFOLLOW); }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    o.open(p)
}

/// Far above any file a store holds — the vectors of a large monorepo run to hundreds of
/// megabytes — and far below what a planted file could make every reader try to hold.
const MAX_STORE_FILE: u64 = 4 << 30;

/// Opens a store file for reading, or `None` when nothing is there. A cloned repository controls
/// every entry under the store, and a link to `/dev/zero` or a FIFO committed there would make
/// every reader read without end, or block on the open; a link at the directory or the file and
/// anything that is not a regular file are refused instead.
pub(crate) fn open_for_read(p: &Path) -> Result<Option<std::fs::File>> {
    if let Some(dir) = p.parent().filter(|d| is_link(d)) {
        anyhow::bail!("{} is a symlink; refusing to read through it — remove it and run again", dir.display());
    }
    match std::fs::symlink_metadata(p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("read {}", p.display())),
        Ok(m) if m.file_type().is_symlink() => {
            anyhow::bail!("{} is a symlink; refusing to read through it — remove it and run again", p.display())
        }
        Ok(m) if !m.is_file() => anyhow::bail!("{} is not a regular file; refusing to read it", p.display()),
        Ok(_) => {}
    }
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    // The checks above name the refusal; the flags close the gap between them and the open, where
    // a swapped-in FIFO would otherwise block the open itself.
    #[cfg(unix)]
    { use std::os::unix::fs::OpenOptionsExt; o.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK); }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let f = o.open(p).with_context(|| format!("read {}", p.display()))?;
    if !f.metadata().with_context(|| format!("read {}", p.display()))?.is_file() {
        anyhow::bail!("{} is not a regular file; refusing to read it", p.display());
    }
    Ok(Some(f))
}

/// The whole of a file `open_for_read` opened, refused past `MAX_STORE_FILE` rather than read.
pub(crate) fn read_capped(f: std::fs::File, p: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let too_big = || anyhow::anyhow!("{} is larger than {MAX_STORE_FILE} bytes; refusing to read it", p.display());
    let len = f.metadata().with_context(|| format!("read {}", p.display()))?.len();
    if len > MAX_STORE_FILE { return Err(too_big()); }
    // Sized up front: a buffer grown by doubling would hold twice the file at its last step.
    let mut bytes = Vec::with_capacity(len as usize);
    // Bounded again while reading: the file may grow past the cap after its length was taken.
    f.take(MAX_STORE_FILE + 1).read_to_end(&mut bytes).with_context(|| format!("read {}", p.display()))?;
    if bytes.len() as u64 > MAX_STORE_FILE { return Err(too_big()); }
    Ok(bytes)
}

fn read_in(dir: &Path, name: &str) -> Result<Option<Vec<u8>>> {
    let p = dir.join(name);
    let Some(f) = open_for_read(&p)? else { return Ok(None) };
    read_capped(f, &p).map(Some)
}

impl Store {
    pub fn new(repo: &Path) -> Store { Store { repo: repo.to_path_buf() } }

    pub(crate) fn dir(&self) -> PathBuf { dir_of(&self.repo) }

    /// Runs `read` against the store where it stands, and once more against `.zerograph` when it
    /// `missed` in a `.repograph` that a writer moved away meanwhile: that is the same store under
    /// its new name, not an absent one.
    fn reading<T>(&self, read: impl Fn(&Path) -> Result<T>, missed: impl Fn(&Result<T>) -> bool) -> Result<T> {
        let dir = self.dir();
        let got = read(&dir);
        let new = self.repo.join(DIR);
        if dir != new && missed(&got) && present(&new) { read(&new) } else { got }
    }

    pub fn load(&self) -> Result<(Graph, Manifest)> { self.load_traced().map(|(g, m, _)| (g, m)) }

    /// Whether a store directory stands under either name. A reader that finds none has nothing
    /// to answer from and creates nothing: the lock it would take makes the directory.
    pub(crate) fn stands(&self) -> bool { dir_of(&self.repo).is_dir() }

    /// [`Manifest::newer_store_line`] when a newer release wrote this store, read leniently: a
    /// manifest that does not parse, or parses only under a newer schema, still names its grammar,
    /// and one that names none is no reason to stop a `build` that rewrites it.
    pub(crate) fn newer_store_line(&self) -> Option<String> {
        let bytes = self.read_bytes("manifest.json").ok().flatten()?;
        let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        let grammar = u32::try_from(v.get("grammar")?.as_u64()?).ok()?;
        let release = v.get("release").and_then(|r| r.as_str()).unwrap_or_default().to_string();
        let m = Manifest { grammar, release, ..Manifest::default() };
        m.newer_grammar().then(|| m.newer_store_line())
    }

    /// The manifest alone, for a writer that must know who wrote the store before it reads the
    /// graph or decides to start from nothing.
    pub fn load_manifest(&self) -> Result<Manifest> {
        Ok(match self.read_bytes("manifest.json")? {
            Some(b) => serde_json::from_slice(&b).context("manifest.json")?,
            None => Manifest::default(),
        })
    }

    /// `load`, saying where the graph came from, so a reader that may write can leave the
    /// mirror a JSON-only store is missing.
    pub fn load_traced(&self) -> Result<(Graph, Manifest, Source)> {
        let (g, source) = self.load_mirrored::<Graph>("graph.json")?;
        Ok((g.unwrap_or_default(), self.load_manifest()?, source))
    }

    /// A JSON store file, read through its mirror when the mirror still describes these bytes.
    pub fn load_mirrored<T: serde::de::DeserializeOwned>(&self, json: &str) -> Result<(Option<T>, Source)> {
        self.reading(|dir| Self::load_mirrored_in(dir, json), |got| !matches!(got, Ok((Some(_), _))))
    }

    fn load_mirrored_in<T: serde::de::DeserializeOwned>(dir: &Path, json: &str) -> Result<(Option<T>, Source)> {
        let p = dir.join(json);
        let Some(now) = stamp(&p) else { return Ok((None, Source::Absent)) };
        if let Ok(Some(bytes)) = read_in(dir, &mirror_name(json)) {
            // A mirror that does not decode, or that the read refuses, is one a crash, another
            // release or a clone left behind; the JSON it mirrors is still there to answer from,
            // so it is passed over, not reported.
            if bytes.starts_with(&mirror_header(now)) {
                if let Ok(v) = postcard::from_bytes::<T>(&bytes[MIRROR_HEADER..]) { return Ok((Some(v), Source::Mirror)); }
            }
        }
        // Stamped from the open handle, not the path: a rename after the open replaces the
        // path's file, never the one being read.
        let Some(f) = open_for_read(&p)? else { return Ok((None, Source::Absent)) };
        let read = f.metadata().ok().as_ref().and_then(stamp_of);
        let bytes = read_capped(f, &p)?;
        let v = serde_json::from_slice(&bytes).context(json.to_string())?;
        Ok((Some(v), Source::Json(MirrorStamp(read))))
    }

    /// The mirror of a JSON file, labelled with the stamp of the bytes `value` came from. A
    /// stamp taken from the path now could be another writer's, and a reader would then decode
    /// this value as that writer's graph.
    pub fn write_mirror<T: serde::Serialize>(&self, json: &str, from: MirrorStamp, value: &T) -> Result<()> {
        let Some(from) = from.0 else { return Ok(()) };
        let mut bytes = mirror_header(from).to_vec();
        bytes.extend(postcard::to_stdvec(value)?);
        self.write_atomic(&mirror_name(json), &bytes)
    }

    /// Whether the store or its lock is a link, which every writer refuses: a reader that finds the
    /// lock refused for this reason writes nothing, where a read-only checkout writes unlocked.
    pub(crate) fn has_link(&self) -> bool {
        is_link(&self.repo.join(DIR)) || is_link(&self.repo.join(LEGACY_DIR)) || is_link(&self.dir().join("writer.lock"))
    }

    /// A store directory that is itself a link is refused for the same reason a link inside it
    /// is: every write and every unlink under it would land wherever the clone pointed it. Both
    /// names, because a write may move the old one onto the new.
    fn refuse_linked_dir(&self) -> Result<()> {
        for dir in [self.repo.join(DIR), self.repo.join(LEGACY_DIR)] {
            if is_link(&dir) {
                anyhow::bail!("{} is a symlink; refusing to write through it — remove it and run again", dir.display());
            }
        }
        Ok(())
    }

    /// The store ready for a write in place, for a process that touches the directory without
    /// going through `ensure_dir` first: neither name a link, and a store under the old name
    /// moved to the new one. Returns where the store is, which is `.zerograph` unless the move
    /// could not be made.
    pub(crate) fn settle(&self) -> Result<PathBuf> {
        self.refuse_linked_dir()?;
        let dir = self.migrate()?;
        // Again after the move: a rename moves a link as readily as a directory.
        self.refuse_linked_dir()?;
        Ok(dir)
    }

    /// Moves a `.repograph` built before the rename to `.zerograph` by one rename in the same
    /// parent — never a copy, never a delete — so whatever stops it, the store is whole under one
    /// name or the other.
    fn migrate(&self) -> Result<PathBuf> {
        let dir = self.repo.join(DIR);
        let legacy = self.repo.join(LEGACY_DIR);
        let legacy_is_dir = || std::fs::symlink_metadata(&legacy).is_ok_and(|m| m.is_dir());
        if present(&dir) || !legacy_is_dir() { return Ok(dir); }
        match std::fs::rename(&legacy, &dir) {
            Ok(()) => Ok(dir),
            // Another process moved it first, or built a new store there: either way it is the one.
            Err(_) if present(&dir) => Ok(dir),
            // Windows refuses to rename a directory while a file inside it is open, which a
            // resident server can keep so for hours. Writing in place keeps the store working,
            // and a later write tries the move again.
            Err(_) if legacy_is_dir() => Ok(legacy),
            Err(e) => Err(e).with_context(|| format!("move {} to {}", legacy.display(), dir.display())),
        }
    }

    /// The store holds passages of the repository's documents and code, so it carries its own `.gitignore`: a `git add -A` in a repository that
    /// never listed `.zerograph/` would otherwise commit it. Returns the directory to write in.
    pub(crate) fn ensure_dir(&self) -> Result<PathBuf> {
        let dir = self.settle()?;
        // The old name comes back only as a directory that stands. Making it again here would put
        // an empty one where another process has just moved the store from, and split it in two.
        if dir != self.repo.join(LEGACY_DIR) {
            std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        }
        let ignore = dir.join(".gitignore");
        match open_unlinked(std::fs::OpenOptions::new().write(true).create_new(true), &ignore) {
            Ok(mut f) => {
                use std::io::Write;
                // An empty file left by a failed write would read as present from then on, and
                // nothing would ever fill it in.
                if let Err(e) = f.write_all(b"*\n") {
                    drop(f);
                    let _ = std::fs::remove_file(&ignore);
                    return Err(e).with_context(|| format!("write {}", ignore.display()));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e).with_context(|| format!("write {}", ignore.display())),
        }
        Ok(dir)
    }

    /// Returns the stamp of the `graph.json` this call wrote, for a writer that claims vectors
    /// against it: a `stat` after the save could already be describing another writer's graph.
    pub fn save(&self, g: &Graph, m: &Manifest) -> Result<Option<crate::walk::Stamp>> {
        self.ensure_dir()?;
        let written = self.write_atomic_stamped("graph.json", &serde_json::to_vec(g)?)?;
        self.write_mirror("graph.json", MirrorStamp(written.as_ref().and_then(stamp_of)), g)?;
        self.save_manifest(m)?;
        Ok(written.as_ref().and_then(crate::walk::stamp_of))
    }

    /// The manifest alone: a walk that found no change can still have learned the stat stamps
    /// that spare the next one from hashing the tree again.
    pub fn save_manifest(&self, m: &Manifest) -> Result<()> {
        self.write_atomic("manifest.json", &serde_json::to_vec(m)?)
    }

    pub fn write_atomic(&self, name: &str, bytes: &[u8]) -> Result<()> {
        self.write_atomic_stamped(name, bytes).map(|_| ())
    }

    /// `write_atomic`, with the stamp of these bytes taken from the temporary file before the
    /// rename, which keeps its mtime and length: a writer renaming over the file a moment later
    /// cannot lend its stamp to them.
    fn write_atomic_stamped(&self, name: &str, bytes: &[u8]) -> Result<Option<std::fs::Metadata>> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = self.ensure_dir()?;
        let (tmp, mut f) = Self::create_tmp(&dir, name, &NEXT)?;
        let written = { use std::io::Write; f.write_all(bytes) }
            .with_context(|| format!("write {}", tmp.display()))
            .and_then(|()| {
                // Closed before the stamp: a platform that settles a file's mtime on close would
                // otherwise stamp bytes the rename then carries under another one.
                drop(f);
                let stamp = std::fs::metadata(&tmp).ok();
                rename_over(&tmp, &dir.join(name)).with_context(|| format!("rename {name}"))?;
                Ok(stamp)
            });
        // Only a file this call created: a name that was taken is skipped in `create_tmp`.
        if written.is_err() { let _ = std::fs::remove_file(&tmp); }
        written
    }

    /// A new temporary file beside `name`, with one name per writer: readers that refresh at once
    /// each write a whole graph, and a shared name let one rename away, or truncate, the file
    /// another was still writing. A name already taken — by what a killed run left under a
    /// recycled pid, or by an entry a clone committed in advance — is skipped, never written
    /// through and never removed: it is not this call's file.
    fn create_tmp(dir: &Path, name: &str, next: &std::sync::atomic::AtomicU64) -> Result<(PathBuf, std::fs::File)> {
        let mut create = std::fs::OpenOptions::new();
        create.write(true).create_new(true);
        for _ in 0..TMP_TRIES {
            let n = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let tmp = dir.join(format!("{name}.{}.{n}.tmp", std::process::id()));
            match open_unlinked(&mut create, &tmp) {
                Ok(f) => return Ok((tmp, f)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists || is_link(&tmp) => continue,
                Err(e) => return Err(e).with_context(|| format!("write {}", tmp.display())),
            }
        }
        anyhow::bail!("write {}: {TMP_TRIES} temporary names in a row are taken", dir.join(name).display())
    }

    /// Extends a store file whose first `keep` bytes the caller still stands behind, dropping
    /// whatever follows them: bytes past that point are what an interrupted write left, and no
    /// metadata names them. Durable before it returns, so the metadata written next never
    /// points at rows a crash could still lose.
    pub fn append_after(&self, name: &str, keep: u64, bytes: &[u8]) -> Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        let p = self.ensure_dir()?.join(name);
        let mut f = open_unlinked(std::fs::OpenOptions::new().write(true).create(true).truncate(false), &p)
            .with_context(|| format!("open {}", p.display()))?;
        f.set_len(keep).with_context(|| format!("truncate {}", p.display()))?;
        f.seek(SeekFrom::Start(keep))?;
        f.write_all(bytes).with_context(|| format!("append {}", p.display()))?;
        f.sync_all()?;
        Ok(())
    }

    /// Whether a store file is there with something in it — a metadata read, so a caller can
    /// decide about a 50 MB file without loading it.
    pub fn has(&self, name: &str) -> bool {
        self.reading(|dir| Ok(std::fs::metadata(dir.join(name)).is_ok_and(|m| m.len() > 0)), |got| matches!(got, Ok(false))).unwrap_or(false)
    }

    /// What a `stat` says about a store file, for a poller that wants to know whether someone
    /// else has written it without reading megabytes to find out.
    pub fn stamp(&self, name: &str) -> Option<crate::walk::Stamp> {
        let at = |dir: &Path| Ok(std::fs::metadata(dir.join(name)).ok().and_then(|m| crate::walk::stamp_of(&m)));
        self.reading(at, |got| matches!(got, Ok(None))).ok().flatten()
    }

    pub fn read_bytes(&self, name: &str) -> Result<Option<Vec<u8>>> {
        self.reading(|dir| read_in(dir, name), |got| !matches!(got, Ok(Some(_))))
    }

    /// A record that has emptied leaves no file behind, so a store never holds one it would not write.
    pub fn remove(&self, name: &str) -> Result<()> {
        let p = self.dir().join(name);
        if p.exists() { std::fs::remove_file(&p).with_context(|| format!("remove {}", p.display()))?; }
        Ok(())
    }

    /// What a writer killed between its write and its rename left behind, and the header record.
    /// The graph and the manifest stay: `build` replaces them by rename when it saves, so a build
    /// that never gets there leaves the previous store to answer from rather than none. A temp file
    /// written in the last few minutes may be another process's save in flight, so only an older
    /// one goes. The headers stay with the graph they were read beside, for the same reason.
    pub fn drop_leftovers(&self) -> Result<()> {
        for e in std::fs::read_dir(self.dir()).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let stale = e.metadata().and_then(|m| m.modified()).ok()
                .and_then(|t| t.elapsed().ok()).is_some_and(|age| age > LEFTOVER_AGE);
            if ["graph.json.", "manifest.json.", "headers.json."].iter().any(|p| name.starts_with(p)) && name.ends_with(".tmp") && stale {
                std::fs::remove_file(e.path()).with_context(|| format!("remove {}", e.path().display()))?;
            }
        }
        Ok(())
    }

    /// The store's writer lock, or `None` when another process holds it. A reader asks this way
    /// and answers from the store as it stands when it is held: the holder is a refresh already
    /// under way, and a second one would only race it to the same rename.
    pub fn try_lock_writer(&self) -> Result<Option<WriterLock>> {
        let f = self.lock_file()?;
        match f.try_lock() {
            Ok(()) => Ok(Some(WriterLock(f))),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) => Err(e).with_context(|| self.lock_label()),
        }
    }

    /// The writer lock, waited for. For a foreground `update` or `build`, whose caller asked for
    /// the store to be brought in line and would rather wait for a running refresh than skip it.
    pub fn lock_writer(&self) -> Result<WriterLock> {
        let f = self.lock_file()?;
        f.lock().with_context(|| self.lock_label())?;
        Ok(WriterLock(f))
    }

    fn lock_label(&self) -> String { format!("lock {}/writer.lock", dir_name_of(&self.repo)) }

    /// Never removed: a lock file deleted while one process holds it lets the next process lock
    /// a new file of the same name, and both believe they are the only writer.
    fn lock_file(&self) -> Result<std::fs::File> {
        let p = self.ensure_dir()?.join("writer.lock");
        open_unlinked(std::fs::OpenOptions::new().create(true).truncate(false).write(true), &p)
            .with_context(|| format!("open {}", p.display()))
    }
}

/// Held for as long as this value lives. The operating system releases it with the handle, so a
/// writer that is killed leaves no lock behind.
pub struct WriterLock(#[allow(dead_code)] std::fs::File);

/// How many temporary names a save tries before it gives up. A name is taken only by a leftover
/// or an entry planted in advance, so the second try is the usual cost and a long run of them is
/// no accident.
const TMP_TRIES: u32 = 64;

/// Far longer than any save takes: a temp file this old has no writer left.
const LEFTOVER_AGE: std::time::Duration = std::time::Duration::from_secs(600);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EdgeKind, Extraction, NodeKind};

    #[cfg(unix)]
    #[test]
    fn a_directory_of_this_users_is_theirs_whatever_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let mine = home.path().join("mine");
        std::fs::create_dir(&mine).unwrap();
        let cache = mine.join("zerograph").join("vectors");
        for mode in [0o755, 0o775, 0o777] {
            std::fs::set_permissions(&mine, std::fs::Permissions::from_mode(mode)).unwrap();
            refuse_shared_ancestors(&cache).unwrap();
        }
        std::fs::set_permissions(&mine, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn the_parents_a_first_run_made_are_private_and_a_second_run_accepts_them() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let cache = home.path().join("a").join("b").join("vectors");
        prepare_private_dir(&refuse_shared_ancestors(&cache).unwrap()).unwrap();
        let mode = std::fs::metadata(home.path().join("a")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "a parent this run made is private to the user");
        // As a umask of 0002 would have left them.
        std::fs::set_permissions(home.path().join("a"), std::fs::Permissions::from_mode(0o775)).unwrap();
        prepare_private_dir(&refuse_shared_ancestors(&cache).unwrap()).unwrap();
    }

    #[test]
    fn an_ancestor_is_refused_when_another_user_owns_it_or_root_lets_others_write_it_unsticky() {
        let (me, them) = (501, 502);
        assert_eq!(others_may_move(me, me, 0o777), None);
        assert!(others_may_move(them, me, 0o755).is_some(), "its owner can rename what is in it");
        assert!(others_may_move(0, me, 0o777).is_some());
        assert!(others_may_move(0, me, 0o775).is_some(), "a group may too");
        assert_eq!(others_may_move(0, me, 0o1777), None, "as /tmp is");
        assert_eq!(others_may_move(0, me, 0o755), None);
    }

    #[test]
    fn a_link_in_the_parent_is_resolved_and_the_path_itself_is_not() {
        let home = tempfile::tempdir().unwrap();
        let real = home.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let alias = home.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let resolved = refuse_shared_ancestors(&alias.join("zerograph").join("vectors")).unwrap();
        assert_eq!(resolved, real.canonicalize().unwrap().join("zerograph").join("vectors"));
        assert_eq!(refuse_shared_ancestors(&alias).unwrap(), home.path().canonicalize().unwrap().join("alias"),
            "the override itself stays a link, for prepare_private_dir to refuse");
    }

    #[test]
    fn the_store_ignores_itself_so_a_commit_of_everything_leaves_it_out() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        drop(store.try_lock_writer().unwrap());
        let dir = d.path().join(".zerograph");
        assert_eq!(std::fs::read_to_string(dir.join(".gitignore")).unwrap(), "*\n");
    }

    /// Two handles in one process contend exactly as two processes do: the lock belongs to the
    /// open file, not to the process, on Unix and on Windows alike.
    #[test]
    fn a_held_writer_lock_is_reported_busy_until_it_is_dropped() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        let held = store.try_lock_writer().unwrap().expect("nobody holds it yet");
        assert!(store.try_lock_writer().unwrap().is_none(), "a second writer is told it is held");
        drop(held);
        // A shell another test spawns in the same instant shares the lock's open file until its
        // exec, so the release can land a moment late: under load the `command` tests made a
        // single attempt fail 4 runs in 30. Waiting for it still fails a lock that is never freed.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while store.try_lock_writer().unwrap().is_none() {
            assert!(std::time::Instant::now() < deadline, "dropping the first frees it");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn graph(label: &str) -> Graph {
        let mut g = Graph::default();
        let mut e = Extraction::default();
        e.node(NodeKind::Requirement, "FR-WEB-01", label, "тело требования", "docs/a.md", 3);
        e.edge("FR-WEB-01", "INV-01", EdgeKind::References, "ctx", "docs/a.md");
        g.apply(e);
        g
    }

    #[test]
    fn a_saved_graph_loads_from_its_mirror_and_matches_the_json() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        let g = graph("Заголовок");
        store.save(&g, &Manifest::default()).unwrap();
        assert!(d.path().join(".zerograph/graph.bin").exists());
        let (g2, _, source) = store.load_traced().unwrap();
        assert_eq!(source, Source::Mirror);
        assert_eq!(g2.nodes, g.nodes);
        assert_eq!(g2.edges, g.edges);
    }

    // The JSON is rewritten to a different length, so the mirror's stamp disagrees even on a
    // file system with coarse mtimes.
    #[test]
    fn a_mirror_behind_a_rewritten_json_is_passed_over() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        store.save(&graph("old"), &Manifest::default()).unwrap();
        store.write_atomic("graph.json", &serde_json::to_vec(&graph("a much longer new label")).unwrap()).unwrap();
        let (g, _, source) = store.load_traced().unwrap();
        assert!(matches!(source, Source::Json(_)));
        assert_eq!(g.nodes["FR-WEB-01"].label, "a much longer new label");
    }

    #[test]
    fn a_mirror_that_does_not_decode_falls_back_to_the_json() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        store.save(&graph("x"), &Manifest::default()).unwrap();
        let p = d.path().join(".zerograph/graph.bin");
        let mut bytes = std::fs::read(&p).unwrap();
        bytes.truncate(MIRROR_HEADER + 3);
        std::fs::write(&p, bytes).unwrap();
        let (g, _, source) = store.load_traced().unwrap();
        assert!(matches!(source, Source::Json(_)));
        assert_eq!(g.nodes.len(), 1);
    }

    #[test]
    fn a_mirror_from_another_release_is_passed_over() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        store.save(&graph("x"), &Manifest::default()).unwrap();
        let p = d.path().join(".zerograph/graph.bin");
        let mut bytes = std::fs::read(&p).unwrap();
        bytes[4..10].copy_from_slice(b"9.9.9\0");
        std::fs::write(&p, bytes).unwrap();
        let (_, _, source) = store.load_traced().unwrap();
        assert!(matches!(source, Source::Json(_)));
    }

    /// A store written by a release whose `Graph` had no `pending`: its JSON reads with none
    /// held aside, and its mirror — a postcard image of the older struct under the older magic
    /// — is passed over rather than decoded into the wrong shape.
    #[test]
    fn a_store_written_before_pending_existed_reads_whole() {
        #[derive(serde::Serialize)]
        struct OldGraph { nodes: std::collections::BTreeMap<String, crate::model::Node>, edges: std::collections::BTreeSet<crate::model::Edge> }
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let mut g = Graph::default();
        let mut e = crate::model::Extraction::default();
        e.node(crate::model::NodeKind::Requirement, "FR-1", "x", "", "docs/a.md", 1);
        e.edge("FR-1", "FR-2", crate::model::EdgeKind::References, "body", "docs/a.md");
        g.apply(e);
        let old = OldGraph { nodes: g.nodes.clone(), edges: g.edges.clone() };
        std::fs::create_dir_all(dir.path().join(".zerograph")).unwrap();
        store.write_atomic("graph.json", &serde_json::to_vec(&old).unwrap()).unwrap();
        store.write_atomic("manifest.json", b"{\"files\":{}}").unwrap();
        // The older release's mirror: its magic, its stamp, its struct.
        let now = stamp(&dir.path().join(".zerograph/graph.json")).unwrap();
        let mut bytes = mirror_header(now).to_vec();
        bytes[..4].copy_from_slice(b"RGM1");
        bytes.extend(postcard::to_stdvec(&old).unwrap());
        store.write_atomic("graph.bin", &bytes).unwrap();

        let (read, _, source) = store.load_traced().unwrap();
        assert!(matches!(source, Source::Json(_)), "the old mirror is passed over, not misread");
        assert_eq!((read.nodes, read.edges), (g.nodes, g.edges));
        assert!(read.pending.is_empty());
    }

    #[test]
    fn a_store_without_a_mirror_reads_the_json_and_can_be_given_one() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        let g = graph("x");
        store.save(&g, &Manifest::default()).unwrap();
        std::fs::remove_file(d.path().join(".zerograph/graph.bin")).unwrap();
        let (_, _, source) = store.load_traced().unwrap();
        let Source::Json(read) = source else { panic!("{source:?}") };
        store.write_mirror("graph.json", read, &g).unwrap();
        let (g2, _, source) = store.load_traced().unwrap();
        assert_eq!(source, Source::Mirror);
        assert_eq!(g2.nodes, g.nodes);
    }

    #[test]
    fn round_trip_and_atomic_write() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        let mut g = Graph::default();
        let mut e = Extraction::default();
        e.node(NodeKind::File, "file:a.md", "a.md", "", "a.md", 1);
        e.edge("file:a.md", "INV-01", EdgeKind::References, "", "a.md");
        g.apply(e);
        let m = Manifest::default();
        store.save(&g, &m).unwrap();
        assert!(d.path().join(".zerograph/graph.json").exists());
        assert_eq!(tmps(d.path()), Vec::<String>::new());
        let (g2, _) = store.load().unwrap();
        assert_eq!(g2.nodes.len(), 1);
        assert_eq!(g2.edges.len(), 1);
    }

    #[test]
    fn corrupt_graph_json_is_an_error_naming_the_file_not_a_panic() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        store.save(&Graph::default(), &Manifest::default()).unwrap();
        std::fs::write(d.path().join(".zerograph/graph.json"), "{ not json").unwrap();
        let err = store.load().unwrap_err().to_string();
        assert!(err.contains("graph.json"), "{err}");
    }

    fn tmps(repo: &Path) -> Vec<String> {
        std::fs::read_dir(repo.join(".zerograph")).unwrap().flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp")).collect()
    }

    #[test]
    fn a_leftover_tmp_from_a_crashed_write_is_not_read_and_is_dropped() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        std::fs::create_dir_all(d.path().join(".zerograph")).unwrap();
        let old = d.path().join(".zerograph/graph.json.1.0.tmp");
        std::fs::write(&old, "garbage").unwrap();
        let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options().write(true).open(&old).unwrap().set_modified(hour_ago).unwrap();
        store.save(&Graph::default(), &Manifest::default()).unwrap();
        let (g, _) = store.load().unwrap();
        assert!(g.nodes.is_empty());
        store.drop_leftovers().unwrap();
        assert_eq!(tmps(d.path()), Vec::<String>::new());
    }

    #[test]
    fn a_temp_file_another_writer_is_saving_right_now_is_kept() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        std::fs::create_dir_all(d.path().join(".zerograph")).unwrap();
        std::fs::write(d.path().join(".zerograph/graph.json.2.0.tmp"), "half a graph").unwrap();
        store.drop_leftovers().unwrap();
        assert_eq!(tmps(d.path()), vec!["graph.json.2.0.tmp".to_string()]);
    }

    // Bytes reach the store only through a rename, so a write that fails part-way — here the
    // store directory refuses new files — cannot leave a half-written graph behind.
    #[cfg(unix)]
    #[test]
    fn a_failed_write_leaves_the_stored_graph_intact() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        let mut g = Graph::default();
        let mut e = Extraction::default();
        e.node(NodeKind::File, "file:a.md", "a.md", "", "a.md", 1);
        g.apply(e);
        store.save(&g, &Manifest::default()).unwrap();
        let dir = d.path().join(".zerograph");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let failed = store.save(&Graph::default(), &Manifest::default()).is_err();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(failed);
        assert_eq!(store.load().unwrap().0.nodes.len(), 1);
    }

    /// Readers that refresh at once each write a whole graph and each rename it into place:
    /// none is refused its rename, and what stands afterwards is one writer's bytes entire.
    #[test]
    fn concurrent_writers_each_publish_a_whole_file() {
        let d = tempfile::tempdir().unwrap();
        let writers: Vec<_> = (0u8..8).map(|w| {
            let repo = d.path().to_path_buf();
            std::thread::spawn(move || {
                let store = Store::new(&repo);
                let bytes = vec![b'a' + w; 256 * 1024];
                for _ in 0..40 { store.write_atomic("graph.json", &bytes).unwrap(); }
            })
        }).collect();
        for w in writers { w.join().unwrap(); }
        let got = Store::new(d.path()).read_bytes("graph.json").unwrap().unwrap();
        assert_eq!(got.len(), 256 * 1024);
        assert!(got.iter().all(|&b| b == got[0]), "a file of two writers' bytes");
        assert_eq!(tmps(d.path()), Vec::<String>::new());
    }

    // The interleaving `ask` can hit: it parsed the JSON, another writer saved, and only then is
    // the mirror written. Labelled with what is on disk now, it would answer every later read
    // with the stale graph.
    #[test]
    fn a_mirror_written_after_another_save_is_passed_over() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        store.save(&graph("first"), &Manifest::default()).unwrap();
        std::fs::remove_file(d.path().join(".zerograph/graph.bin")).unwrap();
        let (stale, _, source) = store.load_traced().unwrap();
        let Source::Json(read) = source else { panic!("{source:?}") };
        store.save(&graph("a second, longer label"), &Manifest::default()).unwrap();
        store.write_mirror("graph.json", read, &stale).unwrap();
        let (g, _, source) = store.load_traced().unwrap();
        assert!(matches!(source, Source::Json(_)));
        assert_eq!(g.nodes["FR-WEB-01"].label, "a second, longer label");
    }

    #[test]
    fn concurrent_saves_never_leave_a_mirror_of_another_writers_json() {
        let d = tempfile::tempdir().unwrap();
        for _ in 0..20 {
            let writers: Vec<_> = ["one", "a much longer two"].into_iter().map(|label| {
                let repo = d.path().to_path_buf();
                std::thread::spawn(move || {
                    let store = Store::new(&repo);
                    for _ in 0..10 { store.save(&graph(label), &Manifest::default()).unwrap(); }
                })
            }).collect();
            for w in writers { w.join().unwrap(); }
            let store = Store::new(d.path());
            let json: Graph = serde_json::from_slice(&store.read_bytes("graph.json").unwrap().unwrap()).unwrap();
            let (g, _, _) = store.load_traced().unwrap();
            assert_eq!(g.nodes["FR-WEB-01"].label, json.nodes["FR-WEB-01"].label);
        }
    }

    #[test]
    fn has_means_present_and_non_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        assert!(!store.has("vectors.f32"));
        store.write_atomic("vectors.f32", b"").unwrap();
        assert!(!store.has("vectors.f32"));
        store.write_atomic("vectors.f32", &[0u8; 8]).unwrap();
        assert!(store.has("vectors.f32"));
    }

    #[test]
    fn dropping_leftovers_keeps_every_store_file() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(d.path());
        s.save(&Graph::default(), &Manifest::default()).unwrap();
        s.write_atomic("vectors.f32", b"v").unwrap();
        s.write_atomic("headers.json", b"{}").unwrap();
        s.drop_leftovers().unwrap();
        assert!(d.path().join(".zerograph/graph.json").exists());
        assert!(d.path().join(".zerograph/manifest.json").exists());
        assert_eq!(s.read_bytes("vectors.f32").unwrap().as_deref(), Some(&b"v"[..]));
        assert_eq!(s.read_bytes("headers.json").unwrap().as_deref(), Some(&b"{}"[..]));
    }

    #[test]
    fn append_after_cuts_the_file_back_to_the_kept_bytes_before_extending_it() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        store.write_atomic("vectors.f32", b"keepGARBAGE").unwrap();
        store.append_after("vectors.f32", 4, b"more").unwrap();
        assert_eq!(store.read_bytes("vectors.f32").unwrap().as_deref(), Some(&b"keepmore"[..]));
    }

    // A cloned repository commits `.zerograph/` whole, links included. Each write a reader or
    // writer makes there must stay inside it: the file outside is left exactly as it was.
    #[cfg(unix)]
    #[test]
    fn a_link_a_clone_committed_in_the_store_is_never_written_through() {
        let d = tempfile::tempdir().unwrap();
        let outside = d.path().join("outside");
        std::fs::write(&outside, "victim\n").unwrap();
        let repo = d.path().join("repo");
        std::fs::create_dir_all(repo.join(".zerograph")).unwrap();
        let store = Store::new(&repo);
        let plant = |name: &str, to: &Path| {
            let link = repo.join(".zerograph").join(name);
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(to, &link).unwrap();
            link
        };
        let link = plant("vectors.f32", &outside);
        assert!(store.append_after("vectors.f32", 0, b"rows").is_err());
        std::fs::remove_file(link).unwrap();
        let link = plant("writer.lock", &outside);
        assert!(store.lock_writer().is_err());
        std::fs::remove_file(link).unwrap();
        // A dangling link reads as absent to `exists`, the case a plain write would have followed.
        let unborn = d.path().join("unborn");
        plant(".gitignore", &unborn);
        assert!(store.ensure_dir().is_err());
        assert!(!unborn.exists());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "victim\n");
    }

    // The check in front of the open refuses a planted link first, so the flag is held to its own
    // test: it is what still stops a link that appears after the check.
    #[cfg(unix)]
    #[test]
    fn the_open_itself_refuses_a_link_when_the_check_before_it_was_beaten() {
        let d = tempfile::tempdir().unwrap();
        let outside = d.path().join("outside");
        std::fs::write(&outside, "victim\n").unwrap();
        let link = d.path().join("link");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(open_nofollow(std::fs::OpenOptions::new().append(true), &link).is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "victim\n");
    }

    // The counter is the test's own: the process-wide one is advanced by every other test that
    // saves, so a name planted ahead of it would never be reached.
    #[cfg(unix)]
    #[test]
    fn a_temporary_name_a_clone_or_a_killed_run_took_is_skipped_and_left_alone() {
        let d = tempfile::tempdir().unwrap();
        let outside = d.path().join("outside");
        std::fs::write(&outside, "victim\n").unwrap();
        let repo = d.path().join("repo");
        let store = Store::new(&repo);
        let dir = store.ensure_dir().unwrap();
        let tmp = |n: u32| repo.join(".zerograph").join(format!("graph.json.{}.{n}.tmp", std::process::id()));
        std::os::unix::fs::symlink(&outside, tmp(0)).unwrap();
        std::fs::write(tmp(1), "left by a killed run").unwrap();
        let (got, file) = Store::create_tmp(&dir, "graph.json",&std::sync::atomic::AtomicU64::new(0)).unwrap();
        drop(file);
        assert_eq!(got, tmp(2));
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "victim\n");
        assert_eq!(std::fs::read_to_string(tmp(1)).unwrap(), "left by a killed run");
        assert!(tmp(0).symlink_metadata().is_ok(), "the link is the clone's to explain, not this call's to remove");
    }

    #[test]
    fn a_save_gives_up_when_every_temporary_name_it_tries_is_taken() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        let dir = store.ensure_dir().unwrap();
        for n in 0..TMP_TRIES {
            std::fs::write(d.path().join(".zerograph").join(format!("graph.json.{}.{n}.tmp", std::process::id())), "x").unwrap();
        }
        let err = Store::create_tmp(&dir, "graph.json", &std::sync::atomic::AtomicU64::new(0)).unwrap_err();
        assert!(format!("{err:#}").contains("temporary names"), "{err:#}");
    }

    #[cfg(unix)]
    #[test]
    fn a_store_directory_that_is_a_link_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let elsewhere = d.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let repo = d.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::os::unix::fs::symlink(&elsewhere, repo.join(".zerograph")).unwrap();
        assert!(Store::new(&repo).save(&Graph::default(), &Manifest::default()).is_err());
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
    }

    /// Every name a reader opens, the mirrors included.
    #[cfg(unix)]
    const STORE_FILES: [&str; 7] =
        ["graph.json", "graph.bin", "manifest.json", "vectors.json", "vectors.f32", "headers.json", "background.log"];

    #[cfg(unix)]
    #[test]
    fn a_device_a_clone_linked_into_the_store_is_refused_rather_than_read_to_its_end() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        store.ensure_dir().unwrap();
        for name in STORE_FILES {
            let link = d.path().join(DIR).join(name);
            std::os::unix::fs::symlink("/dev/zero", &link).unwrap();
            assert!(store.read_bytes(name).is_err(), "{name}");
            std::fs::remove_file(link).unwrap();
        }
        std::os::unix::fs::symlink("/dev/zero", d.path().join(DIR).join("graph.json")).unwrap();
        assert!(store.load().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_mirror_a_clone_linked_away_is_passed_over_and_the_json_answers() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        let g = graph("через зеркало");
        store.save(&g, &Manifest::default()).unwrap();
        let mirror = d.path().join(DIR).join(mirror_name("graph.json"));
        std::fs::remove_file(&mirror).unwrap();
        std::os::unix::fs::symlink("/dev/zero", &mirror).unwrap();
        let (got, source) = store.load_mirrored::<Graph>("graph.json").unwrap();
        assert_eq!(serde_json::to_string(&got.unwrap()).unwrap(), serde_json::to_string(&g).unwrap());
        assert!(matches!(source, Source::Json(_)));
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_in_the_store_is_refused_without_waiting_for_a_writer() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        store.ensure_dir().unwrap();
        for name in STORE_FILES {
            let fifo = d.path().join(DIR).join(name);
            let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
            assert!(store.read_bytes(name).is_err(), "{name}");
            std::fs::remove_file(fifo).unwrap();
        }
    }

    // The type check in front of the open refuses a FIFO first, so the flag is held to its own
    // test: without it, a FIFO swapped in after the check blocks the open until a writer comes.
    #[cfg(unix)]
    #[test]
    fn the_open_itself_does_not_wait_on_a_fifo_when_the_check_before_it_was_beaten() {
        let d = tempfile::tempdir().unwrap();
        let fifo = d.path().join("fifo");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        use std::os::unix::fs::OpenOptionsExt;
        let opened = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(&fifo).unwrap();
        assert!(!opened.metadata().unwrap().is_file());
    }

    #[cfg(unix)]
    #[test]
    fn a_store_directory_that_is_a_link_is_not_read_through() {
        let d = tempfile::tempdir().unwrap();
        let elsewhere = d.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("manifest.json"), "{}").unwrap();
        let repo = d.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::os::unix::fs::symlink(&elsewhere, repo.join(DIR)).unwrap();
        assert!(Store::new(&repo).read_bytes("manifest.json").is_err());
    }

    // Sparse only where the filesystem makes it so; elsewhere the length is written out in full.
    #[cfg(unix)]
    #[test]
    fn a_file_past_the_cap_is_refused_rather_than_held() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vectors.f32");
        let f = std::fs::File::create(&p).unwrap();
        f.set_len(MAX_STORE_FILE + 1).unwrap();
        assert!(read_capped(open_for_read(&p).unwrap().unwrap(), &p).is_err());
    }

    /// A store saved as this release saves it, then put under the name it had before the rename.
    fn legacy_store(repo: &Path) -> Graph {
        let g = graph("до переименования");
        Store::new(repo).save(&g, &Manifest::default()).unwrap();
        Store::new(repo).write_atomic("vectors.f32", b"rows").unwrap();
        std::fs::rename(repo.join(DIR), repo.join(LEGACY_DIR)).unwrap();
        g
    }

    #[test]
    fn a_store_under_the_old_name_is_read_where_it_stands() {
        let d = tempfile::tempdir().unwrap();
        let g = legacy_store(d.path());
        let store = Store::new(d.path());
        let (read, _, source) = store.load_traced().unwrap();
        assert_eq!(read.nodes, g.nodes);
        assert_eq!(source, Source::Mirror);
        assert!(store.has("vectors.f32") && store.stamp("vectors.f32").is_some());
        assert_eq!(store.read_bytes("vectors.f32").unwrap().as_deref(), Some(&b"rows"[..]));
        assert!(!d.path().join(DIR).exists(), "a read moves nothing");
    }

    #[test]
    fn the_first_write_moves_the_old_store_to_the_new_name_whole() {
        let d = tempfile::tempdir().unwrap();
        let g = legacy_store(d.path());
        #[cfg(unix)]
        let inode = { use std::os::unix::fs::MetadataExt; std::fs::metadata(d.path().join(LEGACY_DIR)).unwrap().ino() };
        let store = Store::new(d.path());
        assert_eq!(dir_name_of(d.path()), LEGACY_DIR);
        let _lock = store.lock_writer().unwrap();
        assert_eq!(dir_name_of(d.path()), DIR);
        assert!(!d.path().join(LEGACY_DIR).exists(), "moved, not copied");
        let dir = d.path().join(DIR);
        #[cfg(unix)]
        { use std::os::unix::fs::MetadataExt; assert_eq!(std::fs::metadata(&dir).unwrap().ino(), inode, "the same directory, renamed"); }
        assert_eq!(std::fs::read(dir.join("vectors.f32")).unwrap(), b"rows");
        assert!(dir.join("writer.lock").exists() && dir.join(".gitignore").exists());
        assert_eq!(store.load().unwrap().0.nodes, g.nodes);
    }

    #[test]
    fn with_both_names_present_the_new_one_is_used_and_the_old_one_left_alone() {
        let d = tempfile::tempdir().unwrap();
        let old = legacy_store(d.path());
        let built = tempfile::tempdir().unwrap();
        let g = graph("после");
        Store::new(built.path()).save(&g, &Manifest::default()).unwrap();
        std::fs::rename(built.path().join(DIR), d.path().join(DIR)).unwrap();
        let store = Store::new(d.path());
        assert_eq!(store.load().unwrap().0.nodes, g.nodes);
        store.write_atomic("vectors.f32", b"new rows").unwrap();
        assert_eq!(store.read_bytes("vectors.f32").unwrap().as_deref(), Some(&b"new rows"[..]));
        let legacy = d.path().join(LEGACY_DIR);
        let left: Graph = serde_json::from_slice(&std::fs::read(legacy.join("graph.json")).unwrap()).unwrap();
        assert_eq!(left.nodes, old.nodes, "the old store is never written, moved over or removed");
        assert_eq!(std::fs::read(legacy.join("vectors.f32")).unwrap(), b"rows");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_at_either_name_is_refused_and_nothing_is_moved() {
        let d = tempfile::tempdir().unwrap();
        let elsewhere = d.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("manifest.json"), "{}").unwrap();
        let repo = d.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::os::unix::fs::symlink(&elsewhere, repo.join(LEGACY_DIR)).unwrap();
        let store = Store::new(&repo);
        assert!(store.has_link());
        assert!(store.ensure_dir().is_err());
        assert!(store.lock_writer().is_err());
        assert!(!repo.join(DIR).exists(), "the link was not moved to the new name");
        assert!(repo.join(LEGACY_DIR).symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 1);

        std::fs::remove_file(repo.join(LEGACY_DIR)).unwrap();
        legacy_store(&repo);
        std::os::unix::fs::symlink(&elsewhere, repo.join(DIR)).unwrap();
        assert!(store.has_link());
        assert!(store.save(&Graph::default(), &Manifest::default()).is_err());
        assert!(repo.join(LEGACY_DIR).join("graph.json").exists(), "the old store stays where it was");
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 1);
    }

    #[test]
    fn a_read_that_finds_the_old_store_moved_away_reads_it_under_the_new_name() {
        let d = tempfile::tempdir().unwrap();
        legacy_store(d.path());
        let store = Store::new(d.path());
        let first = std::cell::Cell::new(true);
        // Another process's first write lands between this read resolving the directory and
        // opening the file in it.
        let got = store.reading(|dir| {
            if first.replace(false) {
                assert!(dir.ends_with(LEGACY_DIR));
                std::fs::rename(d.path().join(LEGACY_DIR), d.path().join(DIR)).unwrap();
            }
            read_in(dir, "vectors.f32")
        }, |got| !matches!(got, Ok(Some(_)))).unwrap();
        assert_eq!(got.as_deref(), Some(&b"rows"[..]));
    }

    #[test]
    fn writers_racing_to_move_the_old_store_all_land_in_the_new_one() {
        let d = tempfile::tempdir().unwrap();
        legacy_store(d.path());
        let repo = d.path().to_path_buf();
        let writers: Vec<_> = (0..8).map(|i| {
            let repo = repo.clone();
            std::thread::spawn(move || Store::new(&repo).write_atomic(&format!("w{i}"), b"x"))
        }).collect();
        for w in writers { w.join().unwrap().unwrap(); }
        assert!(!repo.join(LEGACY_DIR).exists());
        for i in 0..8 { assert!(repo.join(DIR).join(format!("w{i}")).exists(), "w{i}"); }
        assert_eq!(std::fs::read(repo.join(DIR).join("vectors.f32")).unwrap(), b"rows");
    }

    #[test]
    fn load_absent_is_empty() {
        let d = tempfile::tempdir().unwrap();
        let (g, m) = Store::new(d.path()).load().unwrap();
        assert!(g.nodes.is_empty() && m.files.is_empty());
    }

    /// A reader that took no delete share — an indexer, a sync client, an editor — holds the
    /// destination for a moment; the rename waits it out rather than failing the save.
    #[cfg(windows)]
    fn hold_open_for(path: &std::path::Path, ms: u64) -> std::thread::JoinHandle<()> {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
        let f = std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(path).unwrap();
        std::thread::spawn(move || { std::thread::sleep(std::time::Duration::from_millis(ms)); drop(f); })
    }

    #[cfg(windows)]
    #[test]
    fn a_rename_over_a_briefly_held_file_waits_and_lands() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        store.write_atomic("graph.json", b"old").unwrap();
        let holder = hold_open_for(&d.path().join(".zerograph/graph.json"), 100);
        store.write_atomic("graph.json", b"new").unwrap();
        holder.join().unwrap();
        assert_eq!(store.read_bytes("graph.json").unwrap().as_deref(), Some(&b"new"[..]));
    }

    /// The error a held destination gives, pinned: 5, not the 32 a sharing violation would be —
    /// the rename is refused where it deletes the file it replaces, and that is reported as
    /// access denied.
    #[cfg(windows)]
    #[test]
    fn a_rename_over_a_file_held_past_the_budget_fails_and_keeps_the_old_bytes() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path());
        store.write_atomic("graph.json", b"old").unwrap();
        let holder = hold_open_for(&d.path().join(".zerograph/graph.json"), 1500);
        let err = store.write_atomic("graph.json", b"new").unwrap_err();
        assert!(format!("{err:#}").contains("os error 5"), "{err:#}");
        holder.join().unwrap();
        assert_eq!(store.read_bytes("graph.json").unwrap().as_deref(), Some(&b"old"[..]));
    }
}
