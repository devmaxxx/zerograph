//! `zerograph serve`: the store, the vectors, the model and the indexes held by one process
//! that answers `ask` over a Unix socket. A fused answer costs a process ~220 ms to open the
//! model (measured on the bench corpus) and 5 ms to use it; a resident one pays the open once.
use crate::{ask, config};
use anyhow::{Context as _, Result};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The socket, one implementation per platform under one name. Both are AF_UNIX at
/// `socket_path`: Windows has spoken it since 10 1803, only through winsock, which std does not
/// wrap, so that side goes through socket2 while unix keeps std's own types. Everything below
/// this module is written once, against `Listener` and `Stream`.
#[cfg(unix)]
mod sys {
    use std::io;
    use std::path::Path;
    pub use std::os::unix::net::{UnixListener as Listener, UnixStream as Stream};
    pub fn bind(path: &Path) -> io::Result<Listener> {
        let l = Listener::bind(path)?;
        // Connecting to a unix socket needs write permission on the file, and the umask decides
        // what a fresh one gets: closed to everyone else before the first accept, not after.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(l)
    }
    pub fn connect(path: &Path) -> io::Result<Stream> { Stream::connect(path) }
    pub fn accept(listener: &Listener) -> io::Result<Stream> { listener.accept().map(|(s, _)| s) }
    /// Whether the name is there at all — a file, not a listener.
    pub fn present(path: &Path) -> bool { path.exists() }
    /// Whether the user running this process owns the file at the name. A socket in a shared
    /// directory that someone else bound is not a server of ours, and its answers would be theirs.
    pub fn ours(path: &Path) -> bool {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        // A link is judged by its own owner, and a repository can ship one that points at another
        // user's socket: only a socket at the name itself counts.
        // SAFETY: geteuid has no preconditions and cannot fail.
        std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_socket() && m.uid() == unsafe { libc::geteuid() })
    }
    /// Whether the process on the other end runs as the user running this one. The socket's mode
    /// already keeps others out; this holds if the file is ever reachable some other way.
    pub fn peer_is_us(stream: &Stream) -> bool {
        use std::os::fd::AsRawFd;
        // SAFETY: geteuid has no preconditions and cannot fail.
        peer_uid(stream.as_raw_fd()).is_some_and(|uid| uid == unsafe { libc::geteuid() })
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn peer_uid(fd: std::os::fd::RawFd) -> Option<libc::uid_t> {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: `cred` and `len` are valid for the size `len` states.
        let rc = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) };
        (rc == 0).then_some(cred.uid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    fn peer_uid(fd: std::os::fd::RawFd) -> Option<libc::uid_t> {
        let (mut uid, mut gid) = (0, 0);
        // SAFETY: both out-pointers are valid for one write.
        (unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } == 0).then_some(uid)
    }
    /// The device and inode of a socket file, which is how a name is told from the thing that was
    /// bound to it: the path can hold a replacement server's socket by the time this one exits.
    pub fn id(path: &Path) -> Option<(u64, u64)> {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(path).ok()?;
        Some((m.dev(), m.ino()))
    }
}

#[cfg(windows)]
mod sys {
    use socket2::{Domain, SockAddr, Socket, Type};
    use std::io;
    use std::path::Path;
    pub type Listener = Socket;
    pub type Stream = Socket;
    pub fn bind(path: &Path) -> io::Result<Listener> {
        let s = Socket::new(Domain::UNIX, Type::STREAM, None)?;
        s.bind(&SockAddr::unix(path)?)?;
        // The backlog std's `UnixListener::bind` asks for on unix.
        s.listen(128)?;
        Ok(s)
    }
    pub fn connect(path: &Path) -> io::Result<Stream> {
        let s = Socket::new(Domain::UNIX, Type::STREAM, None)?;
        s.connect(&SockAddr::unix(path)?)?;
        Ok(s)
    }
    pub fn accept(listener: &Listener) -> io::Result<Stream> { listener.accept().map(|(s, _)| s) }
    /// The per-user temp directory and the profile ACLs are what keep other users out here.
    pub fn ours(_path: &Path) -> bool { true }
    pub fn peer_is_us(_stream: &Stream) -> bool { true }
    /// The socket file is a reparse point with nothing behind it, so the metadata that says it
    /// is there has to be its own, never a target's.
    pub fn present(path: &Path) -> bool { std::fs::symlink_metadata(path).is_ok() }
    /// NTFS's file reference number, sequence included — a record reused for a new file carries
    /// a new sequence, so a name unlinked and bound again is another identity, the way a fresh
    /// inode is on unix. Creation time would not do: NTFS tunnels it, and a file recreated under
    /// a name within fifteen seconds inherits the old one, which is a replacement server's timing.
    ///
    /// `MetadataExt` carries both numbers and has carried them behind an unstable feature since
    /// 2019 (rust-lang/rust#63010), so the handle is asked instead — the open std's own
    /// `symlink_metadata` performs, for the same reason: no access rights, because this is a
    /// metadata question; every share right, because a server is listening on the file while it
    /// is asked; and the reparse point unfollowed, so the numbers are the socket file's own and
    /// never a target's.
    pub fn id(path: &Path) -> Option<(u64, u64)> {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        let file = std::fs::OpenOptions::new()
            .access_mode(0)
            .share_mode(FILE_SHARE_DELETE | FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .ok()?;
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the handle outlives the call, and the struct is only read back where the call
        // reports the kernel filled it in.
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
        if ok == 0 {
            return None;
        }
        Some((
            u64::from(info.dwVolumeSerialNumber),
            u64::from(info.nFileIndexHigh) << 32 | u64::from(info.nFileIndexLow),
        ))
    }
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What a client waits for an answer that is not reranked: a fused one is about 220 ms, and the
/// first of them also opens a 1.3 GB model, so thirty seconds is far longer than an answer costs
/// and still short enough that a wedged server hands the question straight back.
const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// What a client waits for a reranked one instead. `--rerank` is about four seconds of model
/// round-trip; `--rerank-local` is 70-80 s cold on CPU for a 200-candidate pool, most of it the
/// 448 MB cross-encoder session the first such question opens. Under `IO_TIMEOUT` the client gave
/// up on a question the resident process went on to finish and throw away, then scored the same
/// pool itself behind its own cold open — the whole cost a second time, and no `serve:` line to
/// say so. Waiting is strictly cheaper than falling back for anything short of a server that will
/// never answer at all.
const RERANK_TIMEOUT: Duration = Duration::from_secs(300);

/// How long this request's reply is worth waiting for once the server has said it is working on
/// it. Only the read moves: the hello is one line, so the write stays on `IO_TIMEOUT` whatever
/// the question costs to answer.
fn reply_timeout(req: &ask::Request) -> Duration {
    if req.rerank || req.rerank_local { RERANK_TIMEOUT } else { IO_TIMEOUT }
}

/// The line the accept thread writes the instant it takes a connection. It is the only proof a
/// client has that someone is holding its question: the answering loop is single-threaded, so it
/// may be a whole rerank away from reading the hello, and a client that heard nothing would hand
/// its question back after `IO_TIMEOUT` while the server works exactly as intended.
fn ack_line() -> String { serde_json::to_string(&Ack { ack: VERSION.into() }).unwrap() }

/// How often the loop wakes to look at its `--every` and `--idle` deadlines while no question
/// is waiting. No client waits on it — the accept has a thread of its own — so it only has to
/// be finer than the deadlines it guards. An accept polled on a timer instead of blocked would
/// cost every question half that timer: at 50 ms, measured, half a lexical answer.
const WAKE: Duration = Duration::from_millis(250);

/// The socket a repository's server binds and its clients connect to:
/// `~/.cache/zerograph/sock/<hash>.sock`, the hash of the canonical repository path. One place
/// every client finds whatever it is: a socket inside the checkout moved to the temp directory
/// once the checkout sat deep enough, and the temp directory is `$TMPDIR`, which two processes of
/// one user — a shell and an editor's agent — can disagree on, so each found a different server
/// or none. Home is read the way the model cache reads it, through the platform rather than
/// `$HOME`. Server, client and `status` all come here, so no side guesses.
pub fn socket_path(repo: &Path) -> PathBuf {
    socket_path_under(std::env::home_dir().as_deref(), repo)
}

/// `socket_path` against a given home. The name has to fit in `sun_path`, 104 bytes on macOS
/// including the NUL; 100 rather than 104 is one margin for every platform, and four bytes is not
/// worth two numbers. A home too deep for that, or none at all, puts the same name in
/// `fallback_dir`. A relative home is no home: it would put the socket wherever each process
/// happens to stand.
fn socket_path_under(home: Option<&Path>, repo: &Path) -> PathBuf {
    let name = socket_name(repo);
    if let Some(home) = home.filter(|h| h.is_absolute()) {
        let path = home.join(".cache").join("zerograph").join("sock").join(format!("{name}.sock"));
        if path.as_os_str().len() <= 100 { return path; }
    }
    fallback_dir().join(format!("zerograph-{name}.sock"))
}

/// The hash names a file; it defends nothing, which is why it is four lines of FNV rather than a
/// dependency. Canonical, so a checkout reached through a link or a relative `--repo` is one name.
fn socket_name(repo: &Path) -> String {
    let canonical = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in canonical.as_os_str().as_encoded_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Where 0.6.4 bound the socket: `.zerograph/serve.sock` inside the checkout, or the temp
/// directory's `zerograph-<hash>.sock` once that name would not fit in `sun_path`. A server of that
/// release left running across the upgrade listens where no client of this one looks, and goes on
/// polling and writing without the writer lock until its idle timeout. Windows served there too.
fn legacy_socket_path(repo: &Path) -> PathBuf {
    let in_repo = crate::store::dir_of(repo).join("serve.sock");
    if in_repo.as_os_str().len() <= 100 { return in_repo; }
    fallback_dir().join(format!("zerograph-{}.sock", socket_name(repo)))
}

/// Where a socket goes when home cannot hold one. `/tmp` is shared, and a name there that anyone
/// can predict is a name another user can bind first and answer under, so on unix the socket sits
/// in a directory named for the user, which `prepare_socket_dir` makes private. Windows' temp
/// directory is already per-user.
#[cfg(unix)]
fn fallback_dir() -> PathBuf {
    // SAFETY: geteuid has no preconditions and cannot fail.
    std::env::temp_dir().join(format!("zerograph-{}", unsafe { libc::geteuid() }))
}

#[cfg(not(unix))]
fn fallback_dir() -> PathBuf { std::env::temp_dir() }

/// Makes the directory a socket will be bound in, refusing one that is not ours alone: another
/// user may have created the name first, or planted a symlink at it, and a socket bound there is
/// theirs to read and replace. The home cache is held to the same rule as the temp directory —
/// a shared or mis-permissioned home would otherwise hand the socket over just the same. Only the
/// leaf is made private: the directories above it are the user's own to arrange.
#[cfg(unix)]
fn prepare_socket_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    let Some(dir) = path.parent() else { return Ok(()) };
    if let Some(above) = dir.parent() {
        std::fs::create_dir_all(above).with_context(|| format!("create {}", above.display()))?;
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
        anyhow::bail!("{} is not a directory private to this user; remove it and start serve again", dir.display());
    }
    Ok(())
}

/// The profile directory's own ACL is what keeps others out on Windows; all a first `serve` there
/// owes the socket is a directory to be bound in.
#[cfg(not(unix))]
fn prepare_socket_dir(path: &Path) -> Result<()> {
    let Some(dir) = path.parent() else { return Ok(()) };
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))
}

/// What a `stat` says about `zerograph.toml`, or None where there is none — the configuration a
/// one-shot `ask` would read, watched so a resident process cannot answer under an older one.
fn config_stamp(repo: &Path) -> Option<crate::walk::Stamp> {
    crate::walk::stamp_of(&std::fs::metadata(crate::config::project_file(repo)).ok()?)
}

/// What a `stat` says about the executable at this process's own path, or None where it cannot be
/// found. The version alone cannot tell two builds apart: the version moves only at a release,
/// so every development build in between answers the same string, and the README suggests `--idle
/// 86400` — rebuild, ask while yesterday's `serve` is up, and yesterday's code answers with
/// nothing to show for it. Read once per process and held, never per request: `current_exe` names
/// a path, and a rebuild replaces the file at it, so a server that re-read this would answer with
/// the stamp of the very binary the client is asking from and wave that pairing through. A
/// short-lived `ask` reads it at the one instant it runs, which is the same thing. Two copies of
/// one build stamp differently, which costs a fallback and never a wrong answer.
fn build_stamp() -> Option<crate::walk::Stamp> {
    crate::walk::stamp_of(&std::fs::metadata(std::env::current_exe().ok()?).ok()?)
}

/// What a server sends before a reranked answer, as soon as it has read a request it means to
/// answer: the line that separates a server which is working from one that never will. A client
/// waits `IO_TIMEOUT` for it and `RERANK_TIMEOUT` only after it. Its shape shares no required
/// field with `Reply`, so neither can be read as the other, and a server that does not send one
/// — an older build, or any request that is not reranked — answers on the first line as before.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Ack { pub ack: String }

/// The client's half of the handshake. `req.no_dense` is the arm the question was asked in;
/// the server's own arm comes back in the `Reply`, because only the server knows it.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Hello { pub v: String, #[serde(default)] pub build: Option<crate::walk::Stamp>, pub req: ask::Request }

/// The server's half, and its answer. `no_dense` is the arm this process was started in — a
/// server that opened no model can only answer lexically, and a client that asked a fused
/// question has to be told rather than handed a lexical answer under a fused question's name.
/// Both new fields default, so a reply in the older shape still parses. Between releases the
/// version it carries is this one, and what refuses it is the build it does not carry.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Reply {
    pub v: String,
    #[serde(default)] pub build: Option<crate::walk::Stamp>,
    #[serde(default)] pub no_dense: bool,
    pub stdout: String,
    pub stderr: Vec<String>,
    /// The rerank commands this answer started; a server before 0.6.0 sends none.
    #[serde(default)] pub reranked: u32,
    /// Why the answer failed, in which case the client answers in its own process, as it does
    /// when the connection closes unanswered, and `reranked` is what the failure already started.
    #[serde(default)] pub error: Option<String>,
}

/// The resident answer, or None when this process has to answer: no socket, one nobody
/// listens on, another version, another build of this version, a server that cannot answer in
/// the arm the question was asked in, a timeout, or a reply that does not parse.
///
/// Nothing here removes the socket file. A refused connect is a socket nobody listens on — or a
/// live listener whose backlog is full for this instant, and errno does not tell the two apart;
/// deleting the file on that guess would strand a running server holding 1.3 GB, and its own
/// exit would then take the replacement's socket with it. `serve` removes the file it bound
/// itself, and removes a dead one before it binds. All a client owes the question is an answer,
/// and it has one either way.
pub fn try_ask(repo: &Path, req: &ask::Request) -> Option<Reply> {
    try_ask_spending(repo, req).0
}

/// `try_ask`, and the rerank commands a resident answer started before it failed: a model that
/// ran was paid for whether or not an answer came of it, and an allowance has to count it.
pub fn try_ask_spending(repo: &Path, req: &ask::Request) -> (Option<Reply>, u32) {
    match try_ask_within(repo, req, IO_TIMEOUT) {
        Some(Reply { error: Some(e), reranked, .. }) => {
            eprintln!("serve: the resident answer failed: {}; answering here", crate::terminal_safe(&e));
            (None, reranked)
        }
        reply => (reply, 0),
    }
}

/// `try_ask` with the wait for the server's first line named, which is the only thing a test can
/// shorten without the server's cooperation.
fn try_ask_within(repo: &Path, req: &ask::Request, first_line: Duration) -> Option<Reply> {
    let path = socket_path(repo);
    if !sys::present(&path) || !sys::ours(&path) { return None; }
    let mut stream = sys::connect(&path).ok()?;
    // Whatever the question costs the server, its first line is owed straight away.
    stream.set_read_timeout(Some(first_line)).ok()?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).ok()?;
    let build = build_stamp();
    let hello = serde_json::to_string(&Hello { v: VERSION.into(), build, req: req.clone() }).ok()?;
    writeln!(stream, "{hello}").ok()?;
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    // Every connection is acknowledged on accept, so the first line says nothing about this
    // request in particular: read the ack when it is there, and the reply is the line after it.
    if serde_json::from_str::<Ack>(&line).is_ok() {
        // Best-effort: on macOS, a peer that has already written the reply and closed makes this
        // setsockopt fail with EINVAL (the same quirk `hello_line` documents), even though the
        // reply is already sitting in this socket's receive buffer waiting to be read. Losing the
        // ability to extend the wait is not a reason to throw away an answer that already arrived.
        // Set on the reader's own handle: on Windows `try_clone` duplicates the socket, and a
        // timeout set on the original after the clone never reaches the copy that reads.
        let _ = reader.get_ref().set_read_timeout(Some(reply_timeout(req)));
        line.clear();
        reader.read_line(&mut line).ok()?;
    }
    let reply: Reply = serde_json::from_str(&line).ok()?;
    // Said out loud, all three: a question that quietly costs a cold process, or quietly gets a
    // lexical answer, looks like nothing at all. The build and the arm come first because a
    // refusal carries every field and only those two say which of them sent the question back;
    // the version is last because a refusal's own `v` is unmatchable by construction, and this
    // client should print the reason rather than that backstop.
    if reply.build != build {
        eprintln!("serve: the resident process is another build of {VERSION}; answering here");
        return None;
    }
    // This one matters most: a lexical answer to a fused question is an answer, so nothing about
    // it looks wrong. The other direction is not a mismatch — a server holding the model answers
    // a `--no-dense` question lexically, which is what was asked for.
    if reply.no_dense && !req.no_dense {
        eprintln!("serve: the resident process answers lexical-only and this question is fused; answering here");
        return None;
    }
    if reply.v != VERSION {
        eprintln!("serve: the resident process is version {}, this is {VERSION}; answering here", reply.v);
        return None;
    }
    Some(reply)
}

/// What `zerograph stop` sends in place of a hello. It carries no version: a server of any build
/// that knows the line leaves, and one that predates it closes the connection unanswered.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Stop { pub stop: bool }

/// The server's answer to a `Stop`, written before it leaves.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Stopped { pub stopped: String }

/// How long `stop` waits for the socket file to go once the server has said it is leaving: the
/// loop notices within `WAKE`, and a model being dropped is what takes the rest.
const STOP_WAIT: Duration = Duration::from_secs(10);

/// Ends the server answering for this repository through its own exit — the one that removes the
/// socket file — which a kill does not run on Windows. A 0.6.4 server still bound at the old name
/// is ended alongside. False when nothing answers at either.
pub fn stop(repo: &Path) -> Result<bool> {
    // Both before either error is raised: a current server that fails to leave is no reason to
    // leave the old one writing.
    let current = stop_at(&socket_path(repo));
    let legacy = stop_legacy(repo);
    Ok(current? | legacy?)
}

/// The two names coincide only when both fell back to the temp directory, and then the server
/// there has already been asked.
fn stop_legacy(repo: &Path) -> Result<bool> {
    let legacy = legacy_socket_path(repo);
    if legacy == socket_path(repo) { return Ok(false); }
    stop_at(&legacy)
}

fn stop_at(path: &Path) -> Result<bool> {
    if !sys::present(path) || !sys::ours(path) { return Ok(false); }
    let Ok(mut stream) = sys::connect(path) else { return Ok(false) };
    // A server busy with a rerank reads this line when it is done, so the wait is a rerank's.
    stream.set_read_timeout(Some(RERANK_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    writeln!(stream, "{}", serde_json::to_string(&Stop { stop: true })?)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).context("waiting for serve to confirm")? == 0 {
            anyhow::bail!("the server at {} closed without leaving; it predates `stop`, so end the process itself", path.display());
        }
        if serde_json::from_str::<Stopped>(&line).is_ok() { break; }
    }
    let start = Instant::now();
    while sys::present(path) && start.elapsed() < STOP_WAIT { std::thread::sleep(Duration::from_millis(50)); }
    if sys::present(path) { anyhow::bail!("serve said it is leaving, but {} is still there after {}s", path.display(), STOP_WAIT.as_secs()); }
    Ok(true)
}

/// Whether a server of this user's answers at `path`. The connect is closed before any hello,
/// which the server reads as a liveness probe: not a question, and not counted against `--idle`.
pub fn answering(path: &Path) -> bool {
    sys::present(path) && sys::ours(path) && sys::connect(path).is_ok()
}

/// The line `run` logs once it listens, which is where one server's lines in `serve.log` begin.
pub(crate) fn is_start_line(line: &str) -> bool {
    line.starts_with("serve: ") && line.ends_with("; Ctrl-C stops")
}

/// `serve --detach`: the same server in a process of its own, with no console and its output in
/// `serve.log`, and this one gone once the socket answers. Waiting for the bind is what lets the
/// next command be a question rather than a race with the start.
pub fn detach(repo: &Path, no_dense: bool, args: &[String]) -> Result<()> {
    let path = socket_path(repo);
    if sys::present(&path) && sys::connect(&path).is_ok() { anyhow::bail!("another serve answers at {}", path.display()); }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let alive = crate::refresh::spawn_detached_logging(repo, no_dense, false, &args, "serve.log")?;
    let log = format!("{}/serve.log", crate::store::dir_name_of(repo));
    let start = Instant::now();
    loop {
        if sys::present(&path) && sys::connect(&path).is_ok() {
            eprintln!("serve: started in the background; {log} has its output, `zerograph stop` ends it");
            return Ok(());
        }
        if !alive.load(std::sync::atomic::Ordering::SeqCst) { anyhow::bail!("serve exited before it listened; {log} says why"); }
        if start.elapsed() > DETACH_WAIT { anyhow::bail!("serve did not listen within {}s; {log} says why", DETACH_WAIT.as_secs()); }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The bind comes before the model and the vectors open, so this covers a settle and a socket.
const DETACH_WAIT: Duration = Duration::from_secs(60);

pub fn run(repo: &Path, cfg: &config::Config, every: u64, batch: usize, idle: u64, idle_model: u64, no_dense: bool) -> Result<()> {
    // Stamped first of all, because `cfg` was read before this call and the two opens below take
    // seconds on a large store: a file edited inside that window would be stamped as the baseline
    // and this process would then answer under the configuration from before the edit for as long
    // as it ran — the divergence the poll's check exists to prevent, narrowed to a start-up race.
    let cfg_stamp = config_stamp(repo);
    // Read here and carried, for the reason `build_stamp` gives: this process runs the build that
    // was at its path when it started, and nothing it stats later can still say so.
    let build = build_stamp();
    // Before the socket is touched: the unlink and the bind below would otherwise act inside
    // whatever directory a clone pointed `.zerograph` at.
    crate::store::Store::new(repo).settle()?;
    // The old server takes no writer lock, so binding beside it would put two writers on one
    // store. One that will not leave is reported rather than fatal: refusing to start would leave
    // the checkout with only that server.
    match stop_legacy(repo) {
        Ok(true) => eprintln!("serve: stopped a 0.6.4 server at {}", legacy_socket_path(repo).display()),
        Ok(false) => {}
        Err(e) => eprintln!("serve: {e:#}"),
    }
    let path = socket_path(repo);
    prepare_socket_dir(&path)?;
    if sys::present(&path) && sys::connect(&path).is_ok() { anyhow::bail!("another serve answers at {}", path.display()); }
    let _ = std::fs::remove_file(&path);
    let listener = sys::bind(&path).with_context(|| format!("bind {}", path.display()))?;
    // Stamped the instant it exists, and re-read on the way out: the probe above cannot tell a
    // dead socket from a live server whose backlog is momentarily full, so a replacement may
    // have unlinked this file and bound its own at the same name while this one ran. Removing
    // the name rather than the socket would then strand the replacement — the same cascade the
    // client's unlink was deleted to avoid, one process further along.
    let _guard = Unlink(path.clone(), sys::id(&path));
    // After the guard exists, so a signal arriving between the two still finds something to run.
    catch_termination();
    // The accept blocks on a thread of its own and hands each connection over, one at a time —
    // the answer still happens here, on the one thread that holds the context. A rendezvous
    // channel is what keeps it to one: the next connection is accepted but not delivered until
    // this loop is free, and the one after that waits in the kernel's backlog.
    let (tx, rx) = std::sync::mpsc::sync_channel::<sys::Stream>(0);
    std::thread::spawn(move || loop {
        match sys::accept(&listener) {
            // Acknowledged here rather than where the question is answered, because the whole
            // point of the line is to go out while the answering loop is still busy with
            // somebody else's rerank. A probe that connected and closed makes the write fail,
            // which is not this thread's business: the loop below is what decides that a
            // connection carrying no hello asked nothing.
            Ok(mut s) => {
                if !sys::peer_is_us(&s) {
                    eprintln!("serve: refused a connection from another user");
                    continue;
                }
                let _ = s.set_write_timeout(Some(IO_TIMEOUT));
                let _ = writeln!(s, "{}", ack_line());
                if tx.send(s).is_err() { return; }
            }
            Err(_) => return,
        }
    });
    let mut watcher = crate::Watcher::open(repo, cfg)?;
    // Opened stale: the watcher has just walked the tree and owns every later walk. Whatever
    // `open` has to say happened before any request existed, so it is written here rather than
    // handed to whichever client happens to connect first.
    let mut ctx = ask::Context::open(repo, cfg, true, no_dense)?;
    log(&mut ctx);
    // The hook starts this process at session start, and the session's first question follows
    // within seconds: waiting `--every` for the first poll handed that question everything a pull
    // changed — 1469 vectors, 17 s on beauty-crm. Catching up now spends the same time before
    // anyone has asked.
    adopt_if_moved(&mut watcher, &mut ctx, batch)?;
    log(&mut ctx);
    eprintln!("serve: {} every {every}s, batch {batch}, idle {idle}s, model idle {idle_model}s; Ctrl-C stops", path.display());
    let (mut last_poll, mut last_request) = (Instant::now(), Instant::now());
    loop {
        match rx.recv_timeout(WAKE) {
            // Only a question that arrived counts against `--idle`. A bare connect and close is
            // a liveness probe — another `serve` deciding whether to bind, a client that gave
            // up — and treating one as a request keeps this process resident, holding 1.3 GB,
            // for as long as anything at all polls the socket.
            Ok(stream) => match answer(stream, &mut watcher, &mut ctx, build) {
                Ok(asked) => if asked { last_request = Instant::now(); },
                Err(e) => { last_request = Instant::now(); eprintln!("serve: {e:#}"); }
            },
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => anyhow::bail!("the listener stopped accepting"),
        }
        // Ahead of the poll: a poll that embeds a pull can outlast `stop`'s wait, and a stop
        // reported as failed while the server is in fact leaving sends the caller to kill it.
        if terminated() { eprintln!("serve: terminated, exiting"); return Ok(()); }
        if last_poll.elapsed() >= Duration::from_secs(every) {
            last_poll = Instant::now();
            // The configuration is read once, at start: an id family added, a model or a rerank
            // command changed would otherwise divide this process's answers from a one-shot's
            // for as long as it ran. Leaving is the whole of the fix — the next `ask` answers
            // in its own process under the new file, and the next `serve` starts under it too.
            if config_stamp(repo) != cfg_stamp {
                eprintln!("serve: {} changed, exiting — start serve again to answer under it", config::project_file_name(repo));
                return Ok(());
            }
            adopt_if_moved(&mut watcher, &mut ctx, batch)?;
            // A poll between requests answers nobody: its notices are the server's own.
            log(&mut ctx);
        }
        if drop_model_now(idle_model, last_request.elapsed(), ctx.model_open()) {
            ctx.drop_model();
            eprintln!("serve: no question for {idle_model}s, dropped the model — the next fused question pays the open");
        }
        if last_request.elapsed() >= Duration::from_secs(idle) { eprintln!("serve: idle for {idle}s, exiting"); return Ok(()); }
    }
}

/// Whether the weights should go now. Two thresholds read one clock: `--idle` ends the process,
/// `--idle-model` ends only the 1.3 GB behind the dense arm, because a server left up overnight is
/// worth its resident lexical answer and is not worth that. Zero turns the drop off, and a context
/// holding nothing is left alone so the line is printed once per drop rather than every wake.
fn drop_model_now(idle_model: u64, since: Duration, holding: bool) -> bool {
    idle_model > 0 && holding && since >= Duration::from_secs(idle_model)
}

/// A `SIGTERM`ed `serve` used to leave its socket file behind, because the unlink is a `Drop` and
/// a default-handled signal runs none. The handler sets a flag rather than unlinking: the loop
/// wakes every `WAKE` anyway, so a quarter second later it leaves through the same guard as every
/// other exit — the one that checks the socket's identity first, so a replacement server that has
/// already bound this name keeps its socket.
static TERMINATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn terminated() -> bool { TERMINATED.load(std::sync::atomic::Ordering::Relaxed) }

#[cfg(unix)]
extern "C" fn note_term(_sig: libc::c_int) {
    // The one thing a handler may do here: a relaxed store to an atomic is async-signal-safe,
    // where an unlink of a name another process may now own is merely fast.
    TERMINATED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Asks for `SIGTERM` and `SIGINT` to reach `terminated()` instead of killing the process where
/// its `Drop`s cannot run. On Windows there is no equivalent and the socket file is left for the
/// next `serve` to remove; `zerograph stop` is the exit that removes it there.
#[cfg(unix)]
fn catch_termination() {
    for sig in [libc::SIGTERM, libc::SIGINT] {
        unsafe { libc::signal(sig, note_term as *const () as libc::sighandler_t) };
    }
}

#[cfg(not(unix))]
fn catch_termination() {}

/// Notices no client asked for, on the server's stderr.
fn log(ctx: &mut ask::Context) {
    for n in ctx.notices() { eprintln!("{}", crate::terminal_safe(&format!("serve: {n}"))); }
}

/// The poll `ask` does before it answers, and the one case the poll's own answer does not
/// name: a store some other process wrote, which the poll read back into the watcher. Either
/// way the watcher now holds a graph the context does not, and only an adopt leaves a reader
/// where a one-shot `ask` would have been after loading that store from disk.
///
/// Returns the files the tree is ahead of the store by when another refresh held the lock and
/// this poll could apply none of them.
fn adopt_if_moved(watcher: &mut crate::Watcher, ctx: &mut ask::Context, batch: usize) -> Result<usize> {
    let behind = match watcher.poll(batch)? {
        crate::Polled::Refreshed(r) => { ctx.adopt(watcher, Some(r))?; return Ok(0) }
        crate::Polled::Busy { pending } => pending,
        _ => 0,
    };
    if watcher.reloaded { ctx.adopt(watcher, None)?; }
    Ok(behind)
}

/// The client's hello, or None when the peer was gone before it said anything. A connect that
/// closes with nothing on it is a probe, not a question: `serve`'s own check for another server
/// makes exactly that shape, and so does a client that gave up. On macOS a socket with no other
/// end refuses the `setsockopt` below with EINVAL, which reached the server's stderr as an
/// unattributable `serve: Invalid argument (os error 22)`; elsewhere it is an empty read.
fn hello_line(stream: &sys::Stream) -> Option<String> {
    hello_line_within(stream, HELLO_DEADLINE)
}

/// The most a hello may be. A request is a handful of words and flags; a peer sending more is not
/// a client.
pub(crate) const HELLO_MAX: usize = 64 * 1024;

/// How long a client has, in all, to finish its hello line. The serving loop is one thread, so a
/// peer that sends a byte every few seconds would pass any per-read timeout and hold every other
/// question behind it for as long as it liked.
const HELLO_DEADLINE: Duration = Duration::from_secs(10);

fn hello_line_within(stream: &sys::Stream, total: Duration) -> Option<String> {
    stream.set_write_timeout(Some(IO_TIMEOUT)).ok()?;
    let deadline = Instant::now() + total;
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = Vec::new();
    loop {
        let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) else {
            eprintln!("serve: a client did not finish its hello in {}s; not counted as a question", total.as_secs());
            return None;
        };
        // Set on the reader's own handle each round: on Windows `try_clone` duplicates the socket.
        reader.get_ref().set_read_timeout(Some(left)).ok()?;
        let chunk = match reader.fill_buf() {
            Ok(c) => c,
            Err(e) => {
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) {
                    eprintln!("serve: a client connected and said nothing for {}s; not counted as a question", total.as_secs());
                }
                return None;
            }
        };
        if chunk.is_empty() { break; }
        let end = chunk.iter().position(|b| *b == b'\n').map_or(chunk.len(), |i| i + 1);
        line.extend_from_slice(&chunk[..end]);
        reader.consume(end);
        if line.len() > HELLO_MAX { eprintln!("serve: a hello over {HELLO_MAX} bytes; dropped"); return None; }
        if line.ends_with(b"\n") { break; }
    }
    (!line.is_empty()).then(|| String::from_utf8_lossy(&line).into_owned())
}

/// Whether a question was asked, which is what `--idle` counts — a peer that vanished before
/// its hello asked nothing.
fn answer(mut stream: sys::Stream, watcher: &mut crate::Watcher, ctx: &mut ask::Context, build: Option<crate::walk::Stamp>) -> Result<bool> {
    let Some(line) = hello_line(&stream) else { return Ok(false) };
    // Through the same flag a SIGTERM sets, so the exit is the one every other path takes.
    if serde_json::from_str::<Stop>(&line).is_ok_and(|s| s.stop) {
        TERMINATED.store(true, std::sync::atomic::Ordering::Relaxed);
        writeln!(stream, "{}", serde_json::to_string(&Stopped { stopped: VERSION.into() })?)?;
        return Ok(true);
    }
    let hello: Hello = serde_json::from_str(&line).context("hello")?;
    // The refusals, all three under one reply: the build and the arm on it name which of them
    // sent the question back, and the version on it matches no client, present or past, so none
    // can take a refusal for an answer. Answering first and refusing after would spend a fused
    // answer's work on a reply nobody reads.
    if hello.v != VERSION || hello.build != build || (ctx.no_dense() && !hello.req.no_dense) {
        writeln!(stream, "{}", serde_json::to_string(&refusal(ctx, build))?)?;
        return Ok(true);
    }
    // Anything still waiting predates this request — a poll's refresh, an answer that ended in
    // an error. A client is told what its own answer did and nothing else.
    log(ctx);
    ctx.begin();
    if hello.req.stale {
        // `--stale` skips the walk, not the store. A one-shot answers from whatever is on disk
        // at this instant, so a resident one reads the store back when another process has
        // written it — the stat and a load, and none of the walk that was asked to be skipped.
        if watcher.reload_if_moved()? { ctx.adopt(watcher, None)?; }
    } else {
        // The same refresh a one-shot ask does before answering: batch 1 applies any change now.
        // A refresh holding the lock is said the way a one-shot that finds it held says it.
        let behind = adopt_if_moved(watcher, ctx, 1)?;
        if behind > 0 { ctx.note(format!("{}, a refresh is already running", crate::refresh::files_line(behind))); }
    }
    let answered = ctx.answer(&hello.req);
    // Taken before the `?`: a model command started by an answer that then failed was still paid
    // for, and its count must not ride out on the next client's reply.
    let reranked = ctx.take_reranked();
    let stdout = match answered {
        Ok(stdout) => stdout,
        Err(e) => {
            let reply = Reply { reranked, error: Some(format!("{e:#}")), ..header(ctx, build) };
            let _ = writeln!(stream, "{}", serde_json::to_string(&reply)?);
            return Err(e);
        }
    };
    let stderr = ctx.notices();
    let reply = Reply { stdout, stderr, reranked, ..header(ctx, build) };
    writeln!(stream, "{}", serde_json::to_string(&reply)?)?;
    Ok(true)
}

/// What this process is, with no answer in it: the three fields a client decides on.
fn header(ctx: &ask::Context, build: Option<crate::walk::Stamp>) -> Reply {
    Reply { v: VERSION.into(), build, no_dense: ctx.no_dense(), stdout: String::new(), stderr: vec![], reranked: 0, error: None }
}

/// The same header under a version no build carries. An empty `stdout` is a refusal here and a
/// perfectly good answer to a question nothing matched, and the two are told apart by fields a
/// client older than this path never reads: such a client checks `v`, matches, prints nothing and
/// exits 0 — the silent non-answer the refusal was written to stop, one binary generation on. A
/// version it cannot match sends it back to its own process instead, and the fields a current
/// client reads first still name the reason.
fn refusal(ctx: &ask::Context, build: Option<crate::walk::Stamp>) -> Reply {
    Reply { v: format!("{VERSION} (refused)"), ..header(ctx, build) }
}

struct Unlink(PathBuf, Option<(u64, u64)>);
impl Drop for Unlink {
    fn drop(&mut self) {
        if self.1.is_some() && sys::id(&self.0) == self.1 { let _ = std::fs::remove_file(&self.0); }
    }
}

#[cfg(test)]
mod tests {
    use super::{ack_line, build_stamp, drop_model_now, reply_timeout, socket_path, sys, try_ask, try_ask_spending, try_ask_within, Ack, Reply, IO_TIMEOUT, RERANK_TIMEOUT, VERSION};
    use crate::ask::Request;
    use std::io::{BufRead, BufReader, Write};
    use std::time::Duration;

    /// The bug behind a `--rerank-local` question that never printed a `serve:` line: the client
    /// timed out at thirty seconds on an answer that takes seventy, threw away the resident one
    /// and paid a second cold open to answer it again. `tests/serve.rs` cannot reach this — there
    /// is no reranker model on the runner — so the choice is pinned here.
    #[test]
    fn a_reranked_question_is_waited_out_and_a_plain_one_is_not() {
        let plain = Request { words: vec!["чаевые".into()], json: false, seeds: 5, bodies: false, rerank: false, rerank_local: false, depth: crate::rerank::DEPTH, stale: false, no_dense: false };
        assert_eq!(reply_timeout(&plain), IO_TIMEOUT);
        assert_eq!(reply_timeout(&Request { rerank_local: true, ..plain.clone() }), RERANK_TIMEOUT);
        assert_eq!(reply_timeout(&Request { rerank: true, ..plain.clone() }), RERANK_TIMEOUT);
        assert!(crate::rerank::DEADLINE < RERANK_TIMEOUT, "a stopped model command answers before the client stops waiting");
    }

    /// `--idle` exits, `--idle-model` forgets — two thresholds off the one clock, so a server
    /// left up overnight keeps the 6.8 ms lexical answer and not the 1.3 GB behind the dense one.
    #[test]
    fn the_weights_go_on_their_own_idle_and_only_while_something_is_held() {
        assert!(!drop_model_now(0, Duration::from_secs(9_999), true), "zero is off, at any age");
        assert!(!drop_model_now(300, Duration::from_secs(299), true));
        assert!(drop_model_now(300, Duration::from_secs(300), true), "the threshold is reached, not passed");
        assert!(!drop_model_now(300, Duration::from_secs(301), false),
                "nothing held: the line would otherwise print on every wake for the rest of the day");
    }


    /// A socket bound where `socket_path` puts it, for a test that plays the server: the directory
    /// made the way `run` makes it, and the file gone when the test is, since the name lives in
    /// the user's own cache rather than in the test's temp directory.
    struct Bound(std::path::PathBuf);

    impl Bound {
        fn at(repo: &std::path::Path) -> (sys::Listener, Bound) {
            let path = socket_path(repo);
            super::prepare_socket_dir(&path).unwrap();
            let _ = std::fs::remove_file(&path);
            (sys::bind(&path).unwrap(), Bound(path))
        }
    }

    impl Drop for Bound {
        fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); }
    }

    /// An absolute path on the platform the test runs on: `/home` is relative on Windows, and a
    /// relative home is no home.
    fn rooted(rest: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" }).join(rest)
    }

    /// One place every client finds: a checkout's socket does not move with how deep the checkout
    /// sits, nor with a `TMPDIR` two processes of one user can disagree on.
    #[test]
    fn a_repository_gets_its_socket_under_the_user_cache() {
        let home = &rooted("home/u");
        let repo = std::path::Path::new("/private/tmp").join("a".repeat(120));
        let p = super::socket_path_under(Some(home), &repo);
        assert_eq!(p.parent().unwrap(), home.join(".cache/zerograph/sock"), "{}", p.display());
        assert!(p.extension().is_some_and(|e| e == "sock"), "{}", p.display());
        assert_eq!(super::socket_path_under(Some(home), &repo), p, "a client that computes it again finds the same name");
        assert_ne!(super::socket_path_under(Some(home), &repo.join("x")), p, "another repository is another socket");
    }

    /// Where 0.6.4 bound, so a server of that release left running across the upgrade can still be
    /// told to leave: inside the checkout while the name fits `sun_path`, the temp directory's
    /// `zerograph-<hash>.sock` once it does not.
    #[test]
    fn the_socket_a_0_6_4_server_bound_is_where_that_release_put_it() {
        let shallow = std::path::Path::new("/tmp/r");
        assert_eq!(super::legacy_socket_path(shallow), crate::store::dir_of(shallow).join("serve.sock"));
        let deep = std::path::Path::new("/tmp").join("a".repeat(120));
        let p = super::legacy_socket_path(&deep);
        assert_eq!(p.parent().unwrap(), super::fallback_dir(), "{}", p.display());
        assert_eq!(p.file_name().unwrap().to_string_lossy(), format!("zerograph-{}.sock", super::socket_name(&deep)));
    }

    /// Through the platform's idea of home, as the model cache is: a Windows console sets no `$HOME`.
    #[test]
    fn the_socket_of_a_real_repository_is_under_the_home_the_platform_names() {
        let repo = std::path::Path::new("/tmp/r");
        assert_eq!(socket_path(repo), super::socket_path_under(std::env::home_dir().as_deref(), repo));
    }

    /// `sun_path` is 104 bytes on macOS, including the NUL, so a home deep enough cannot hold a
    /// socket at all; and with no home there is nowhere of the user's own to look. Either way the
    /// name moves to a directory of the user's own under the temp directory, under the same hash.
    #[test]
    fn a_socket_that_would_not_fit_under_home_goes_to_the_private_temp_directory() {
        let deep_home = rooted("home").join("h".repeat(80));
        let repo = std::path::Path::new("/tmp/r");
        let in_home = super::socket_path_under(Some(&rooted("home/u")), repo);
        for p in [super::socket_path_under(Some(&deep_home), repo), super::socket_path_under(None, repo)] {
            assert!(p.as_os_str().len() <= 100, "{}", p.display());
            assert_eq!(p.parent().unwrap(), super::fallback_dir(), "{}", p.display());
            assert_eq!(p.file_name().unwrap().to_string_lossy(), format!("zerograph-{}", in_home.file_name().unwrap().to_string_lossy()),
                       "one hash, whichever directory it lands in");
        }
        #[cfg(unix)]
        assert!(super::fallback_dir().ends_with(format!("zerograph-{}", unsafe { libc::geteuid() })), "the fallback is a directory of the user's own");
    }

    /// A home that is not absolute would put the socket wherever the process happens to stand, and
    /// two processes in two directories would never meet.
    #[test]
    fn a_relative_home_is_no_home() {
        let p = super::socket_path_under(Some(std::path::Path::new("rel")), std::path::Path::new("/tmp/r"));
        assert_eq!(p.parent().unwrap(), super::fallback_dir(), "{}", p.display());
    }

    /// `serve` once unlinked a stale socket inside `.zerograph` before anything else touched the
    /// store, so a `.zerograph` that is a link was acted on before `ensure_dir` could refuse it.
    /// The socket has left the store since, and the store is still refused before any of it.
    #[cfg(unix)]
    #[test]
    fn a_store_directory_that_is_a_link_is_refused_before_any_socket_is_touched() {
        let d = tempfile::Builder::new().tempdir_in("/tmp").unwrap();
        let outside = d.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("serve.sock"), "victim\n").unwrap();
        let repo = d.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::os::unix::fs::symlink(&outside, repo.join(".zerograph")).unwrap();
        let sock = socket_path(&repo);
        assert!(!sock.starts_with(&repo) && !sock.starts_with(&outside), "the socket does not follow the store: {}", sock.display());
        let err = super::run(&repo, &crate::config::Config::default(), 1, 1, 1, 0, true).unwrap_err();
        assert!(format!("{err:#}").contains("symlink"), "{err:#}");
        assert_eq!(std::fs::read_to_string(outside.join("serve.sock")).unwrap(), "victim\n");
        assert!(!sys::present(&sock), "nothing was bound for a store that was refused");
    }

    /// The two facts `try_ask` and `Unlink` read off the socket file, through the platform's own
    /// metadata: on Windows the file is a reparse point, and `exists` would ask what it points at.
    #[test]
    fn a_bound_socket_file_is_present_and_carries_an_identity_until_it_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("serve.sock");
        assert!(!sys::present(&path) && sys::id(&path).is_none());
        let first = sys::bind(&path).unwrap();
        assert!(sys::present(&path));
        let id = sys::id(&path).expect("a bound socket file has an identity");
        drop(first);
        std::fs::remove_file(&path).unwrap();
        assert!(!sys::present(&path), "the name goes with the file");
        let _second = sys::bind(&path).unwrap();
        // NTFS bumps a reused record's sequence number, so the new file is a new identity by
        // construction. A unix filesystem may hand the inode straight back, so there this is not
        // asserted — and the guard's dev+ino check has always lived with that.
        if cfg!(windows) { assert_ne!(sys::id(&path), Some(id), "bound again under the name, another file"); }
    }

    /// The long wait is for a server that is reranking, and a server that accepted the connection
    /// and will never answer looks exactly like one until it says otherwise. `serve` is an
    /// accelerator: a client may not do worse with one than without, and five silent minutes is
    /// as much worse as this protocol can get.
    #[test]
    fn a_server_that_never_answers_hands_a_reranked_question_back_on_the_first_line() {
        let dir = tempfile::tempdir().unwrap();
        let (listener, _bound) = Bound::at(dir.path());
        // Accepts, reads nothing, replies nothing -- a wedged worker, a process under a debugger,
        // a reranker child that hung.
        let held = std::thread::spawn(move || {
            let accepted = sys::accept(&listener);
            std::thread::sleep(Duration::from_secs(2));
            drop(accepted);
        });
        let req = Request { words: vec!["чаевые".into()], json: false, seeds: 5, bodies: false, rerank: true, rerank_local: false, depth: crate::rerank::DEPTH, stale: false, no_dense: false };
        let started = std::time::Instant::now();
        assert!(try_ask_within(dir.path(), &req, Duration::from_millis(300)).is_none(), "nothing came back to parse");
        assert!(started.elapsed() < Duration::from_secs(2), "fell back after {:?}", started.elapsed());
        let _ = held.join();
    }

    /// The queue is the case the acknowledgement exists for: a server busy with somebody else's
    /// rerank has not read this hello yet, and the only thing keeping this client from falling
    /// back at `IO_TIMEOUT` is a line written by the thread that accepted the connection.
    #[test]
    fn a_connection_acknowledged_before_its_hello_is_read_keeps_the_long_wait() {
        let dir = tempfile::tempdir().unwrap();
        let (listener, _bound) = Bound::at(dir.path());
        let build = build_stamp();
        let held = std::thread::spawn(move || {
            let mut s = sys::accept(&listener).unwrap();
            writeln!(s, "{}", ack_line()).unwrap();
            // Busy elsewhere for longer than the shortened first-line wait below.
            std::thread::sleep(Duration::from_millis(700));
            let mut line = String::new();
            BufReader::new(s.try_clone().unwrap()).read_line(&mut line).unwrap();
            let reply = Reply { v: VERSION.into(), build, no_dense: false, stdout: "queued".into(), stderr: vec![], reranked: 0, error: None };
            writeln!(s, "{}", serde_json::to_string(&reply).unwrap()).unwrap();
        });
        let req = Request { words: vec!["чаевые".into()], json: false, seeds: 5, bodies: false, rerank: true, rerank_local: false, depth: crate::rerank::DEPTH, stale: false, no_dense: false };
        let got = try_ask_within(dir.path(), &req, Duration::from_millis(300));
        held.join().unwrap();
        assert_eq!(got.map(|r| r.stdout), Some("queued".into()), "an acknowledged client waits out the queue it is in");
    }

    /// The extension only matters if a reply that was not yet on the wire when the ack was read
    /// is still picked up afterwards: the ack buys the wait, and this is what the wait is for.
    #[test]
    fn a_reply_that_arrives_after_the_ack_still_extends_the_wait_to_find_it() {
        let dir = tempfile::tempdir().unwrap();
        let (listener, _bound) = Bound::at(dir.path());
        let build = build_stamp();
        let held = std::thread::spawn(move || {
            let mut s = sys::accept(&listener).unwrap();
            let mut line = String::new();
            BufReader::new(s.try_clone().unwrap()).read_line(&mut line).unwrap();
            writeln!(s, "{}", ack_line()).unwrap();
            // Later than the shortened first-line wait below, so the read that finds it only
            // succeeds if the ack actually moved the deadline out to `reply_timeout`.
            std::thread::sleep(Duration::from_millis(700));
            let reply = Reply { v: VERSION.into(), build, no_dense: false, stdout: "hi".into(), stderr: vec![], reranked: 0, error: None };
            writeln!(s, "{}", serde_json::to_string(&reply).unwrap()).unwrap();
        });
        let req = Request { words: vec!["чаевые".into()], json: false, seeds: 5, bodies: false, rerank: true, rerank_local: false, depth: crate::rerank::DEPTH, stale: false, no_dense: false };
        let got = try_ask_within(dir.path(), &req, Duration::from_millis(300));
        held.join().unwrap();
        assert_eq!(got.map(|r| r.stdout), Some("hi".into()), "the late reply should still have been read after the ack extended the timeout");
    }

    /// A failed answer is no answer, but the model it started was paid for: the client answers
    /// itself and is told the count, so an allowance charges it.
    #[test]
    fn a_failed_resident_answer_is_handed_back_with_what_it_spent() {
        let dir = tempfile::tempdir().unwrap();
        let (listener, _bound) = Bound::at(dir.path());
        let build = build_stamp();
        let held = std::thread::spawn(move || {
            let mut s = sys::accept(&listener).unwrap();
            let mut line = String::new();
            BufReader::new(s.try_clone().unwrap()).read_line(&mut line).unwrap();
            writeln!(s, "{}", ack_line()).unwrap();
            let reply = Reply { v: VERSION.into(), build, no_dense: false, stdout: String::new(), stderr: vec![], reranked: 1, error: Some("the store went away".into()) };
            writeln!(s, "{}", serde_json::to_string(&reply).unwrap()).unwrap();
        });
        let req = Request { words: vec!["чаевые".into()], json: false, seeds: 5, bodies: false, rerank: true, rerank_local: false, depth: crate::rerank::DEPTH, stale: false, no_dense: false };
        let (got, spent) = try_ask_spending(dir.path(), &req);
        held.join().unwrap();
        assert!(got.is_none(), "a failed answer is answered here instead");
        assert_eq!(spent, 1);
    }

    /// On macOS, `setsockopt` for a read timeout fails with EINVAL once the peer has already
    /// written everything and closed — the same quirk `hello_line` documents, met here from the
    /// other side of the same socket. A server that answers fast enough to write its reply and
    /// return (dropping the connection) before the client gets to extend its own timeout must
    /// not cost the client an answer that already fully arrived.
    #[test]
    fn a_reply_already_behind_a_closed_peer_is_read_even_when_extending_the_timeout_fails() {
        let dir = tempfile::tempdir().unwrap();
        let (listener, _bound) = Bound::at(dir.path());
        let build = build_stamp();
        let held = std::thread::spawn(move || {
            let mut s = sys::accept(&listener).unwrap();
            let mut line = String::new();
            BufReader::new(s.try_clone().unwrap()).read_line(&mut line).unwrap();
            writeln!(s, "{}", ack_line()).unwrap();
            // No delay: the reply and the drop of `s` below race the client's own extension of
            // its read timeout, and on macOS this side wins often enough to pin the bug on.
            let reply = Reply { v: VERSION.into(), build, no_dense: false, stdout: "hi2".into(), stderr: vec![], reranked: 0, error: None };
            writeln!(s, "{}", serde_json::to_string(&reply).unwrap()).unwrap();
        });
        let req = Request { words: vec!["чаевые".into()], json: false, seeds: 5, bodies: false, rerank: true, rerank_local: false, depth: crate::rerank::DEPTH, stale: false, no_dense: false };
        let got = try_ask(dir.path(), &req);
        held.join().unwrap();
        assert_eq!(got.map(|r| r.stdout), Some("hi2".into()), "a reply that already fully arrived must not be thrown away over a timeout that could not be extended");
    }

    /// The two lines a client reads share no required field, so neither can be taken for the
    /// other: a reply on the first line is an older server's answer and is read as one.
    #[test]
    fn an_acknowledgement_and_a_reply_are_never_read_as_each_other() {
        let ack = serde_json::to_string(&Ack { ack: "0.5.3".into() }).unwrap();
        let reply = serde_json::to_string(&Reply {
            v: "0.5.3".into(), build: None, no_dense: false, stdout: String::new(), stderr: vec![], reranked: 0, error: None,
        }).unwrap();
        assert!(serde_json::from_str::<Reply>(&ack).is_err(), "{ack}");
        assert!(serde_json::from_str::<Ack>(&reply).is_err(), "{reply}");
    }

    /// What `run` does on the way out: the listener is still alive on its thread when the guard
    /// removes the file. If the platform refused this, the cost would be a stale file for the
    /// next `serve` to sweep — worth knowing rather than guessing.
    #[test]
    fn the_socket_file_is_removed_while_its_listener_is_still_bound() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("serve.sock");
        let _listener = sys::bind(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(!sys::present(&path));
    }

    /// The whole reason for a socket file over a port: a name nobody listens on is settled at
    /// once, not accepted by a stranger and waited out. Windows and Linux refuse the connect;
    /// macOS sometimes completes it against the file a dropped listener left and hands back a
    /// stream with nothing behind it, which reads as end of file — what `try_ask` does with a
    /// reply it cannot parse is answer in its own process, so the outcome is the same either way.
    #[test]
    fn a_connect_to_a_socket_nobody_listens_on_is_settled_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("serve.sock");
        drop(sys::bind(&path).unwrap());
        let started = std::time::Instant::now();
        match sys::connect(&path) {
            Err(err) => assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused, "{err}"),
            Ok(stream) => {
                if cfg!(windows) { panic!("a socket nobody listens on is refused on Windows, and this connect was taken"); }
                stream.set_read_timeout(Some(std::time::Duration::from_secs(1))).unwrap();
                let mut line = String::new();
                let read = BufReader::new(stream).read_line(&mut line);
                assert!(matches!(read, Ok(0) | Err(_)), "nothing is behind it: {read:?} {line:?}");
            }
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "settled after {:?}", started.elapsed());
    }

    /// A user's name is in the socket's path, and on Windows the path crosses into `sun_path` as
    /// UTF-8: two bytes a Cyrillic letter, and a conversion the runner's own ASCII path never makes.
    #[test]
    fn a_socket_binds_and_connects_under_a_directory_that_is_not_ascii() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("Максим");
        std::fs::create_dir(&home).unwrap();
        let path = home.join("serve.sock");
        let _listener = sys::bind(&path).unwrap();
        assert!(sys::present(&path));
        sys::connect(&path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_bound_socket_is_closed_to_everyone_but_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("serve.sock");
        let _l = sys::bind(&path).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(sys::ours(&path));
    }

    #[cfg(unix)]
    #[test]
    fn a_peer_of_the_same_user_passes_the_uid_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("serve.sock");
        let l = sys::bind(&path).unwrap();
        let _c = sys::connect(&path).unwrap();
        assert!(sys::peer_is_us(&sys::accept(&l).unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn a_fallback_directory_that_is_not_private_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let sock = super::socket_path_under(None, std::path::Path::new("/tmp/b"));
        let dir = sock.parent().unwrap().to_path_buf();
        super::prepare_socket_dir(&sock).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let refused = super::prepare_socket_dir(&sock);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(refused.is_err(), "a group- or world-accessible directory is not ours alone");
    }

    /// The home cache is held to the fallback's rule: a shared or mis-permissioned home would
    /// otherwise hand the socket to whoever else can reach the directory.
    #[cfg(unix)]
    #[test]
    fn a_home_socket_directory_is_made_private_and_refused_when_it_is_not() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::Builder::new().tempdir_in("/tmp").unwrap();
        let sock = super::socket_path_under(Some(home.path()), std::path::Path::new("/tmp/r"));
        let dir = sock.parent().unwrap().to_path_buf();
        assert_eq!(dir, home.path().join(".cache/zerograph/sock"), "{}", sock.display());
        super::prepare_socket_dir(&sock).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700, "made private where it did not exist");
        super::prepare_socket_dir(&sock).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert!(super::prepare_socket_dir(&sock).is_err(), "a group-readable socket directory is not ours alone");
        std::fs::remove_dir(&dir).unwrap();
        std::os::unix::fs::symlink(home.path(), &dir).unwrap();
        assert!(super::prepare_socket_dir(&sock).is_err(), "a link in the directory's place is not a directory of ours");
    }

    /// A peer that sends a byte at a time, each inside the per-read timeout, is a hold on the
    /// one thread that answers everybody.
    #[test]
    fn a_peer_trickling_its_hello_is_dropped_at_the_total_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("serve.sock");
        let l = sys::bind(&path).unwrap();
        let trickle = std::thread::spawn(move || {
            let mut c = sys::connect(&path).unwrap();
            for _ in 0..40 {
                if c.write_all(b"x").is_err() { return; }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let s = sys::accept(&l).unwrap();
        let started = std::time::Instant::now();
        assert!(super::hello_line_within(&s, Duration::from_millis(400)).is_none());
        assert!(started.elapsed() < Duration::from_millis(1500), "held for {:?}", started.elapsed());
        drop(s);
        let _ = trickle.join();
    }

    #[test]
    fn a_hello_that_arrives_in_pieces_in_time_is_read_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("serve.sock");
        let l = sys::bind(&path).unwrap();
        let c = std::thread::spawn(move || {
            let mut c = sys::connect(&path).unwrap();
            c.write_all(b"{\"a\"").unwrap();
            std::thread::sleep(Duration::from_millis(100));
            c.write_all(b":1}\n").unwrap();
        });
        let s = sys::accept(&l).unwrap();
        assert_eq!(super::hello_line_within(&s, Duration::from_secs(5)).as_deref(), Some("{\"a\":1}\n"));
        c.join().unwrap();
    }
}
