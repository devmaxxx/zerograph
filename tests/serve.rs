use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The crate is a binary with no library, so the test carries its own copy of the transport seam
/// `src/serve.rs` keeps: std's Unix socket where std has one, socket2's AF_UNIX on Windows.
#[cfg(unix)]
mod transport {
    use std::path::Path;
    pub use std::os::unix::net::{UnixListener as Listener, UnixStream as Stream};
    pub fn bind(path: &Path) -> Listener { Listener::bind(path).unwrap() }
    pub fn connect(path: &Path) -> std::io::Result<Stream> { Stream::connect(path) }
    pub fn accept(listener: &Listener) -> Stream { listener.accept().unwrap().0 }
}

#[cfg(windows)]
mod transport {
    use socket2::{Domain, SockAddr, Socket, Type};
    use std::path::Path;
    pub type Listener = Socket;
    pub type Stream = Socket;
    pub fn bind(path: &Path) -> Listener {
        let s = Socket::new(Domain::UNIX, Type::STREAM, None).unwrap();
        s.bind(&SockAddr::unix(path).unwrap()).unwrap();
        s.listen(1).unwrap();
        s
    }
    pub fn connect(path: &Path) -> std::io::Result<Stream> {
        let s = Socket::new(Domain::UNIX, Type::STREAM, None)?;
        s.connect(&SockAddr::unix(path)?)?;
        Ok(s)
    }
    pub fn accept(listener: &Listener) -> Stream { listener.accept().unwrap().0 }
}

/// A home of the tests' own, shared by every child of this binary so a client and its server agree
/// on it: the socket lives under the user's cache, and a server a failed or killed test leaves
/// behind would otherwise leave its file in the real one. Under `/tmp` on unix because a macOS
/// temp directory is deep enough to push the socket past `sun_path` into the fallback.
fn test_home() -> &'static std::path::Path {
    static HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    HOME.get_or_init(|| if cfg!(unix) { tempfile::tempdir_in("/tmp") } else { tempfile::tempdir() }.unwrap()).path()
}

/// Every child this file spawns, less the two variables the bench kit exports. `common::run`
/// subtracts the same pair and says why; these tests do not go through it, because half of them
/// spawn a copy of the binary from a path of their own — which is the case they exist for — and a
/// shell left over from a measurement would answer them under other weights all the same.
fn child(path: &std::path::Path) -> Command {
    let mut c = Command::new(path);
    c.env("HOME", test_home()).env("USERPROFILE", test_home());
    c.env_remove("ZEROGRAPH_BENCH_REPO").env_remove("REPOGRAPH_BENCH_REPO").env_remove("ZEROGRAPH_EMBED_MODEL").env_remove("REPOGRAPH_EMBED_MODEL").env_remove("ZEROGRAPH_CODE_GLOBS").env_remove("REPOGRAPH_CODE_GLOBS");
    c
}

fn zerograph() -> Command { child(std::path::Path::new(env!("CARGO_BIN_EXE_zerograph"))) }

/// Where this repository's server listens, in the binary's own words: `status --json` names the
/// socket it checks, which is the one `serve` binds and `ask` connects to. The crate is a binary
/// with no library, so asking it is how a test learns the name without a second copy of the rule.
fn socket_of(dir: &std::path::Path) -> std::path::PathBuf {
    let out = zerograph().arg("--repo").arg(dir).args(["status", "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    std::path::PathBuf::from(v["serve"]["socket"].as_str().unwrap_or_else(|| panic!("status names no socket: {v}")))
}

/// A stand-in server's socket, bound where the binary looks. Its directory is made as `serve`
/// makes it — private on unix, or the next real `serve` would refuse it — and the file goes with
/// the test, since the name lives in the user's cache rather than in the test's temp directory.
struct Fake(std::path::PathBuf);

impl Fake {
    fn bind(dir: &std::path::Path) -> (transport::Listener, Fake) {
        let sock = socket_of(dir);
        let parent = sock.parent().unwrap();
        std::fs::create_dir_all(parent.parent().unwrap()).unwrap();
        #[cfg(unix)]
        match std::os::unix::fs::DirBuilderExt::mode(&mut std::fs::DirBuilder::new(), 0o700).create(parent) {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => panic!("{}: {e}", parent.display()),
            _ => {}
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(parent).unwrap();
        let _ = std::fs::remove_file(&sock);
        (transport::bind(&sock), Fake(sock))
    }
}

impl Drop for Fake {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); }
}

fn repo_with_docs() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    std::fs::write(dir.path().join("docs/pay.md"), "**FR-PAY-1 · MUST · Штраф за отмену**\n\nШтраф списывается сам (INV-1).\n\n**INV-1 · MUST · Деньги не сгорают**\n\nОтмена не сжигает деньги.\n").unwrap();
    std::fs::write(dir.path().join("docs/cal.md"), "**FR-CAL-1 · MUST · Перенос визита**\n\nПеренос не считается отменой.\n").unwrap();
    std::fs::write(dir.path().join("zerograph.toml"), "id_families = [\"FR-PAY\", \"FR-CAL\", \"INV\"]\n").unwrap();
    let out = zerograph().args(["--no-dense", "--repo"]).arg(dir.path()).arg("build").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    dir
}

/// The arm a command is run in. The repo these tests build has no vectors, so the fused arm
/// never opens the 448 MB model here — what it changes is the flag the handshake is decided on.
const LEXICAL: &[&str] = &["--no-dense"];
const FUSED: &[&str] = &[];

fn ask_in(arm: &[&str], dir: &std::path::Path, extra: &[&str], words: &[&str]) -> (String, String) {
    let out = zerograph().args(arm).arg("--repo").arg(dir).arg("ask").args(extra).args(words).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    (String::from_utf8(out.stdout).unwrap(), String::from_utf8(out.stderr).unwrap())
}

fn ask(dir: &std::path::Path, extra: &[&str], words: &[&str]) -> (String, String) {
    ask_in(LEXICAL, dir, extra, words)
}

/// The same question asked by a chosen binary rather than the one Cargo built, so a test can
/// replace the file under a running server and still ask from the path it replaced.
#[cfg(unix)]
fn ask_from(bin: &std::path::Path, dir: &std::path::Path, words: &[&str]) -> (String, String) {
    let out = child(bin).args(LEXICAL).arg("--repo").arg(dir).arg("ask").args(words).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    (String::from_utf8(out.stdout).unwrap(), String::from_utf8(out.stderr).unwrap())
}

/// The two numbers the handshake's build stamp is made of, read the way `walk::stamp_of` reads
/// them. The crate is a binary with no library, so a test computes them rather than calling it.
fn stamp_of(path: &std::path::Path) -> (u64, u64) {
    let m = std::fs::metadata(path).unwrap();
    let ns = m.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    (u64::try_from(ns).unwrap(), m.len())
}

/// A hello in the shape the wire carries it, carrying no build — a stamp no server can match, so
/// every server refuses it. Which of the three refusals it is does not matter to the tests below:
/// what they are about is the shape of a refusal reply, which is one reply for all three.
fn hello_with_no_build() -> String {
    format!(
        concat!(r#"{{"v":"{}","build":null,"req":{{"words":["штраф"],"json":false,"seeds":12,"#,
                r#""bodies":false,"rerank":false,"rerank_local":false,"depth":2,"stale":true,"no_dense":true}}}}"#),
        env!("CARGO_PKG_VERSION"))
}

/// The server's own reply, read straight off the socket with no `ask` in between. Asked until one
/// comes rather than slept on: a socket file says a `serve` bound, only a reply says it listens.
/// The acknowledgement the accept thread writes is skipped here — it is not a reply, and every
/// connection carries one.
fn reply_to(dir: &std::path::Path, hello: &str) -> String {
    use std::io::{BufRead, Write};
    let sock = socket_of(dir);
    let start = Instant::now();
    loop {
        if let Ok(mut s) = transport::connect(&sock) {
            s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
            if writeln!(s, "{hello}").is_ok() {
                let mut reader = std::io::BufReader::new(&s);
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    if !line.contains("\"ack\"") { return line; }
                    line.clear();
                }
            }
        }
        assert!(start.elapsed() < Duration::from_secs(20), "serve never replied on its socket");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A `serve` child that ends with the test and takes its socket file along. A killed server leaves
/// the file behind, which used to go with the test's temp directory; the name lives in the user's
/// cache now, and a run of this file would otherwise leave one there per kill.
struct Server { child: std::process::Child, sock: std::path::PathBuf }

impl Server {
    /// The name is asked for before the start, so the asking is not a probe the new server sees.
    fn start(dir: &std::path::Path, cmd: &mut Command) -> Server {
        let sock = socket_of(dir);
        Server { child: cmd.stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap(), sock }
    }
}

impl std::ops::Deref for Server {
    type Target = std::process::Child;
    fn deref(&self) -> &std::process::Child { &self.child }
}

impl std::ops::DerefMut for Server {
    fn deref_mut(&mut self) -> &mut std::process::Child { &mut self.child }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.sock);
    }
}

fn serve_in(arm: &[&str], dir: &std::path::Path, extra: &[&str]) -> Server {
    Server::start(dir, zerograph().args(arm).arg("--repo").arg(dir).arg("serve").args(extra))
}

fn serve(dir: &std::path::Path, extra: &[&str]) -> Server {
    serve_in(LEXICAL, dir, extra)
}

/// The answer once a server is the one giving it. A socket file that exists says a `serve` has
/// bound; only a reply says it is listening, so this asks until one comes rather than sleeping.
fn ask_until_resident(dir: &std::path::Path, words: &[&str]) -> (String, String) {
    let start = Instant::now();
    loop {
        let (out, err) = ask(dir, &[], words);
        if err.contains("serve: answered by the resident process") { return (out, err); }
        assert!(start.elapsed() < Duration::from_secs(20), "serve never answered over the socket: {err}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_socket(dir: &std::path::Path) {
    let sock = socket_of(dir);
    let start = Instant::now();
    // A socket file is a reparse point on Windows, and `exists` would follow it to nowhere; its
    // own metadata is what says it is there, on every platform.
    while std::fs::symlink_metadata(&sock).is_err() {
        assert!(start.elapsed() < Duration::from_secs(20), "serve never opened its socket");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn the_socket_answers_the_bytes_the_process_answers_and_sees_an_edit() {
    let dir = repo_with_docs();
    let questions = [vec!["штраф"], vec!["перенос", "визита"], vec!["FR-CAL-1"], vec!["ничего"]];
    let direct: Vec<_> = questions.iter().map(|q| ask(dir.path(), &["--no-serve"], q).0).collect();
    // Polling every hour, so the edit below can only be seen by the refresh before the answer —
    // the behaviour this test is named for, rather than a background poll that beat it to it.
    let mut server = serve(dir.path(), &["--every", "3600", "--idle", "60"]);
    wait_for_socket(dir.path());
    for (q, want) in questions.iter().zip(&direct) {
        let (got, err) = ask(dir.path(), &[], q);
        assert_eq!(&got, want, "question {q:?}");
        assert!(err.contains("serve: answered by the resident process"), "the client says it answered through the socket: {err}");
    }
    let json_direct = ask(dir.path(), &["--no-serve", "--json"], &["штраф"]).0;
    assert_eq!(ask(dir.path(), &["--json"], &["штраф"]).0, json_direct);
    std::fs::write(dir.path().join("docs/new.md"), "**FR-PAY-2 · MUST · Возврат аванса**\n\nАванс возвращается при отмене салоном.\n").unwrap();
    let (after, _) = ask(dir.path(), &[], &["возврат", "аванса"]);
    assert!(after.contains("FR-PAY-2"), "the server refreshed before answering: {after}");
    server.kill().unwrap();
    let _ = server.wait();
}

#[test]
fn a_server_catches_up_with_the_tree_at_start_rather_than_at_its_first_poll() {
    let dir = repo_with_docs();
    std::fs::write(dir.path().join("docs/new.md"), "**FR-PAY-2 · MUST · Возврат аванса**\n\nАванс возвращается при отмене салоном.\n").unwrap();
    let mut server = serve(dir.path(), &["--every", "3600", "--idle", "60"]);
    wait_for_socket(dir.path());
    // `--stale` skips the refresh before the answer, so only the start-up catch-up can have
    // brought the new document in.
    let start = Instant::now();
    let (after, err) = loop {
        let (out, err) = ask(dir.path(), &["--stale"], &["возврат", "аванса"]);
        if err.contains("serve: answered by the resident process") { break (out, err); }
        assert!(start.elapsed() < Duration::from_secs(20), "serve never answered over the socket: {err}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(after.contains("FR-PAY-2"), "the server refreshed on start: {after}\n{err}");
    server.kill().unwrap();
    let _ = server.wait();
}

#[test]
fn a_socket_nobody_listens_on_is_answered_here_and_left_for_the_next_serve() {
    let dir = repo_with_docs();
    let (_, fake) = Fake::bind(dir.path());
    // The listener is dropped at once: the file stays, nothing accepts.
    let started = Instant::now();
    let (out, err) = ask(dir.path(), &[], &["штраф"]);
    // A connect nobody accepts is refused at once on every platform this runs on. One that were
    // accepted by nothing would sit out the client's whole read timeout, and a resident answer
    // exists to be faster than a process, not thirty seconds slower than one.
    assert!(started.elapsed() < Duration::from_secs(5), "the fallback took {:?}", started.elapsed());
    let want = ask(dir.path(), &["--no-serve"], &["штраф"]).0;
    assert_eq!(out, want);
    assert!(out.contains("FR-PAY-1"));
    assert!(!err.contains("serve:"), "{err}");
    // A client that cannot be answered falls back; it does not delete files. A refused connect
    // is also what a live server with a full backlog gives, and unlinking on that guess would
    // strand it. `serve` owns the file, and binds over a dead one.
    assert!(std::fs::symlink_metadata(&fake.0).is_ok(), "the client leaves the socket file alone");
    let mut server = serve(dir.path(), &["--every", "3600", "--idle", "60"]);
    let (through, _) = ask_until_resident(dir.path(), &["штраф"]);
    assert_eq!(through, want);
    server.kill().unwrap();
    let _ = server.wait();
}

#[test]
fn an_edited_config_stops_the_server_rather_than_answering_under_the_old_one() {
    let dir = repo_with_docs();
    let mut server = serve(dir.path(), &["--every", "1", "--idle", "60"]);
    wait_for_socket(dir.path());
    std::fs::write(dir.path().join("zerograph.toml"), "id_families = [\"FR-PAY\", \"FR-CAL\", \"INV\", \"NFR\"]\n").unwrap();
    let start = Instant::now();
    loop {
        if let Some(status) = server.try_wait().unwrap() {
            assert!(status.success(), "{status}");
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "the server kept answering under the old config");
        std::thread::sleep(Duration::from_millis(50));
    }
    // And the question is answered here, under the file as it now reads.
    assert!(ask(dir.path(), &[], &["штраф"]).0.contains("FR-PAY-1"));
}

/// The resident process writes the store, so it may not extract under a staler family set than
/// an `update` would. A family added to a document while it runs used to be read with the matcher
/// it built at start-up: the file yielded no node, and its new hash went into the manifest, so no
/// later update ever read it again.
#[test]
fn a_family_a_document_grows_while_the_server_runs_reaches_the_store_it_writes() {
    let dir = repo_with_docs();
    let mut server = serve(dir.path(), &["--every", "1", "--idle", "60"]);
    wait_for_socket(dir.path());
    ask_until_resident(dir.path(), &["штраф"]);
    std::fs::write(dir.path().join("docs/new.md"), "**NEW-1 · MUST · Новое правило**\n\nтело правила.\n").unwrap();
    let start = Instant::now();
    loop {
        // Read from the store on disk, which is the server's own writing: a poll that extracted
        // the file under the old set would leave a `file:` node and nothing else.
        let (out, _) = ask(dir.path(), &["--no-serve", "--stale"], &["NEW-1"]);
        if out.lines().next().is_some_and(|l| l.starts_with("docs/new.md:1  NEW-1  ")) { break; }
        assert!(start.elapsed() < Duration::from_secs(20), "the server never wrote NEW-1: {out}");
        std::thread::sleep(Duration::from_millis(100));
    }
    server.kill().unwrap();
    let _ = server.wait();
}

#[test]
fn a_stale_socket_answer_is_no_older_than_the_store_on_disk() {
    let dir = repo_with_docs();
    // The one poll this server will do for the whole test happened at start-up: whatever it
    // learns later, it learns because a request made it look, not because a timer did.
    let mut server = serve(dir.path(), &["--every", "3600", "--idle", "60"]);
    wait_for_socket(dir.path());
    assert!(!ask_until_resident(dir.path(), &["возврат", "аванса"]).0.contains("FR-PAY-2"));
    std::fs::write(dir.path().join("docs/new.md"), "**FR-PAY-2 · MUST · Возврат аванса**\n\nАванс возвращается при отмене салоном.\n").unwrap();
    // Another process writes the store — the walk is its, not the server's.
    let want = ask(dir.path(), &["--no-serve"], &["возврат", "аванса"]).0;
    assert!(want.contains("FR-PAY-2"), "{want}");
    let (got, err) = ask(dir.path(), &["--stale"], &["возврат", "аванса"]);
    assert!(err.contains("serve: answered by the resident process"), "{err}");
    assert_eq!(got, ask(dir.path(), &["--no-serve", "--stale"], &["возврат", "аванса"]).0);
    assert!(got.contains("FR-PAY-2"), "a stale answer still reads the store on disk: {got}");
    server.kill().unwrap();
    let _ = server.wait();
}

#[test]
fn a_reply_from_another_version_is_ignored() {
    let dir = repo_with_docs();
    let (listener, _fake) = Fake::bind(dir.path());
    // Stamped as the asking binary is, so the version is the only field left to refuse it on —
    // the client reads the build and the arm first, and a reply that failed those would prove
    // nothing about the check this test is named for.
    let (mtime_ns, len) = stamp_of(std::path::Path::new(env!("CARGO_BIN_EXE_zerograph")));
    let fake = std::thread::spawn(move || {
        use std::io::{BufRead, Write};
        let mut s = transport::accept(&listener);
        let mut line = String::new();
        std::io::BufReader::new(s.try_clone().unwrap()).read_line(&mut line).unwrap();
        writeln!(s, r#"{{"v":"0.0.0","build":{{"mtime_ns":{mtime_ns},"len":{len}}},"no_dense":true,"stdout":"WRONG\n","stderr":[]}}"#).unwrap();
    });
    let (out, err) = ask(dir.path(), &[], &["штраф"]);
    assert!(out.contains("FR-PAY-1") && !out.contains("WRONG"), "{out}");
    assert!(err.contains("serve: the resident process is version 0.0.0"), "{err}");
    fake.join().unwrap();
}

// The two tests below are the pairing the socket gate could not see for as long as every test
// passed `--no-dense` on both sides: a server's arm and a client's are two facts, and only one
// of the four pairings diverges.
#[test]
fn a_fused_question_is_refused_by_a_server_that_answers_lexical_only() {
    let dir = repo_with_docs();
    let want = ask_in(FUSED, dir.path(), &["--no-serve"], &["штраф"]).0;
    let mut server = serve(dir.path(), &["--every", "3600", "--idle", "60"]);
    wait_for_socket(dir.path());
    // Asked until the refusal comes rather than once: a socket file says a `serve` bound, and
    // only a reply says it is listening. The line asserted on has one producer, the handshake,
    // so a server that is merely not up yet cannot pass this by falling back for another reason.
    let start = Instant::now();
    let (got, err) = loop {
        let (got, err) = ask_in(FUSED, dir.path(), &[], &["штраф"]);
        if err.contains("serve: the resident process answers lexical-only") { break (got, err); }
        assert!(start.elapsed() < Duration::from_secs(20), "a fused question was answered lexical-only and told nothing: {err}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(!err.contains("answered by the resident process"), "{err}");
    assert_eq!(got, want, "the client answered it here, as it would have with no server at all");
    server.kill().unwrap();
    let _ = server.wait();
}

#[test]
fn a_lexical_question_is_still_answered_by_a_server_holding_the_dense_arm() {
    let dir = repo_with_docs();
    let want = ask(dir.path(), &["--no-serve"], &["штраф"]).0;
    let mut server = serve_in(FUSED, dir.path(), &["--every", "3600", "--idle", "60"]);
    let (through, _) = ask_until_resident(dir.path(), &["штраф"]);
    assert_eq!(through, want, "a server that opened the model narrows to the arm it was asked in");
    server.kill().unwrap();
    let _ = server.wait();
}

// Every development build between two releases answers the same version, so it alone would let
// yesterday's `serve` answer today's question — under `--idle 86400`, for a day.
#[test]
fn a_reply_from_another_build_of_this_version_is_ignored() {
    let dir = repo_with_docs();
    let (listener, _fake) = Fake::bind(dir.path());
    let fake = std::thread::spawn(move || {
        use std::io::{BufRead, Write};
        let mut s = transport::accept(&listener);
        let mut line = String::new();
        std::io::BufReader::new(s.try_clone().unwrap()).read_line(&mut line).unwrap();
        // This version, and a stamp no executable on this machine carries.
        writeln!(s, r#"{{"v":"{}","build":{{"mtime_ns":1,"len":1}},"no_dense":false,"stdout":"WRONG\n","stderr":[]}}"#, env!("CARGO_PKG_VERSION")).unwrap();
    });
    let (out, err) = ask(dir.path(), &[], &["штраф"]);
    assert!(out.contains("FR-PAY-1") && !out.contains("WRONG"), "{out}");
    assert!(err.contains("serve: the resident process is another build"), "{err}");
    fake.join().unwrap();
}

// Unix only: Windows will not replace a running image, and a rebuild there fails until the
// server stops, so the pairing this test refuses cannot be made.
/// `current_exe` names a path, and the file at a server's own path is the one a rebuild replaces.
/// A server that stats it per request reports the stamp of the binary the client is asking from,
/// the two agree, and yesterday's code answers today's question — the case the build stamp exists
/// to refuse. Only a stamp taken once, at start, describes the build a process is running.
#[cfg(unix)]
#[test]
fn a_binary_replaced_under_a_live_server_is_another_build_and_the_client_answers_here() {
    let dir = repo_with_docs();
    let bin = tempfile::tempdir().unwrap();
    let path = bin.path().join("zerograph");
    std::fs::copy(env!("CARGO_BIN_EXE_zerograph"), &path).unwrap();
    let started_as = stamp_of(&path);
    let want = ask(dir.path(), &["--no-serve"], &["штраф"]).0;
    let mut server = Server::start(dir.path(), child(&path).args(LEXICAL).arg("--repo").arg(dir.path())
        .args(["serve", "--every", "3600", "--idle", "60"]));
    // The one build on both ends answers over the socket, which is what makes the run below a
    // test of the replacement rather than of a server that was never reachable.
    let start = Instant::now();
    loop {
        let (_, err) = ask_from(&path, dir.path(), &["штраф"]);
        if err.contains("serve: answered by the resident process") { break; }
        assert!(start.elapsed() < Duration::from_secs(20), "serve never answered over the socket: {err}");
        std::thread::sleep(Duration::from_millis(50));
    }
    // A rebuild in place, as far as a stat can tell: the path now holds a file the running server
    // did not start from. Renamed rather than written over — the running one keeps its inode.
    let replacement = bin.path().join("replacement");
    std::fs::copy(env!("CARGO_BIN_EXE_zerograph"), &replacement).unwrap();
    // `fs::copy` carries the source's mtime over on macOS, so the copy would stamp as the file it
    // came from. A build writes its output now, and that is the stamp under test.
    std::fs::File::options().write(true).open(&replacement).unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now())).unwrap();
    std::fs::rename(&replacement, &path).unwrap();
    assert_ne!(started_as, stamp_of(&path), "the file at that path stamps differently now");
    let (out, err) = ask_from(&path, dir.path(), &["штраф"]);
    assert!(err.contains("serve: the resident process is another build"),
        "the running server is not the build at its path, and the client says so: {err}");
    assert!(!err.contains("answered by the resident process"), "{err}");
    assert_eq!(out, want, "and the question was answered here instead");
    server.kill().unwrap();
    let _ = server.wait();
}

/// The refusal itself, with no `ask` between it and the assertion: a reply carrying the three
/// fields a client decides on, no answer, and a version no build has.
#[test]
fn a_hello_the_server_will_not_answer_is_refused_with_a_header_and_no_answer() {
    let dir = repo_with_docs();
    let mut server = serve(dir.path(), &["--every", "3600", "--idle", "60"]);
    let line = reply_to(dir.path(), &hello_with_no_build());
    let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(reply["stdout"], "", "a refusal carries no answer: {line}");
    assert_eq!(reply["no_dense"], true, "and still says which arm this server holds: {line}");
    assert!(reply["build"].is_object(), "and which build it is running: {line}");
    assert!(reply["v"].as_str().unwrap().starts_with(env!("CARGO_PKG_VERSION")), "{line}");
    server.kill().unwrap();
    let _ = server.wait();
}

/// A client older than the refusal path checks nothing but `v`. Handed a refusal it could match,
/// it would print an empty stdout and exit 0 — no answer, no line, nothing to see: the same
/// silent failure the refusal was written to stop, one binary generation on. No client, present
/// or past, may take a refusal for an answer, so a refusal carries a version no build has.
#[test]
fn a_client_that_checks_only_the_version_cannot_take_a_refusal_for_an_answer() {
    let dir = repo_with_docs();
    let mut server = serve(dir.path(), &["--every", "3600", "--idle", "60"]);
    let line = reply_to(dir.path(), &hello_with_no_build());
    let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
    // What such a client would have printed, and the check that is all it has to stop it.
    assert_eq!(reply["stdout"].as_str().unwrap(), "", "the answer it would have printed: {line}");
    assert_ne!(reply["v"].as_str().unwrap(), env!("CARGO_PKG_VERSION"),
        "the one check an older client makes sends it back to its own process: {line}");
    server.kill().unwrap();
    let _ = server.wait();
}

/// Eight questions in flight at once against one server. The accept blocks on its own thread and
/// hands connections over a rendezvous channel one at a time, so seven of the eight are waiting
/// somewhere — in the channel, or in the kernel's backlog — while the first is answered. A
/// platform that refused a concurrent connect would still answer all eight correctly, in eight
/// cold processes, and the only thing that tells that apart is the line each client prints.
#[test]
fn several_asks_at_once_are_all_answered_by_the_one_resident_process() {
    let dir = repo_with_docs();
    let mut server = serve(dir.path(), &["--every", "3600", "--idle", "60"]);
    let (want, _) = ask_until_resident(dir.path(), &["штраф"]);
    let asks: Vec<_> = (0..8).map(|_| {
        zerograph().args(LEXICAL).arg("--repo").arg(dir.path()).args(["ask", "штраф"])
            .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap()
    }).collect();
    for (i, child) in asks.into_iter().enumerate() {
        let out = child.wait_with_output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(out.status.success(), "ask {i}: {err}");
        assert_eq!(String::from_utf8(out.stdout).unwrap(), want, "ask {i} answered something else");
        assert!(err.contains("serve: answered by the resident process"), "ask {i} answered itself: {err}");
    }
    server.kill().unwrap();
    let _ = server.wait();
}

/// A second `serve` while the first is answering. Its probe is a connect and a close — the one
/// shape a running server must survive — and what it decides is whether it binds over a live
/// socket, which would strand the first behind a name nothing reaches and take that name with it
/// on the way out. So the second has to refuse, and the first has to still be there afterwards.
#[test]
fn a_second_serve_on_the_same_repository_bails_and_leaves_the_first_answering() {
    let dir = repo_with_docs();
    let mut first = serve(dir.path(), &["--every", "3600", "--idle", "60"]);
    let (want, _) = ask_until_resident(dir.path(), &["штраф"]);
    // Idle seconds rather than minutes: this call is waited on, and a second server that wrongly
    // bound would otherwise hold the test for as long as the first one's own `--idle`.
    let second = zerograph().args(LEXICAL).arg("--repo").arg(dir.path())
        .args(["serve", "--every", "3600", "--idle", "2"]).output().unwrap();
    let err = String::from_utf8_lossy(&second.stderr).into_owned();
    assert!(!second.status.success(), "a second serve bound over a live one: {err}");
    assert!(err.contains("another serve answers"), "{err}");
    let (again, err) = ask(dir.path(), &[], &["штраф"]);
    assert!(err.contains("serve: answered by the resident process"), "the probe cost the first server its socket: {err}");
    assert_eq!(again, want);
    first.kill().unwrap();
    let _ = first.wait();
}

/// `--idle` through a whole process, which is the only thing that runs the guard: the socket file
/// is removed on the way out by a `Drop` that first asks whether the file at the name is still
/// the one this process bound. An identity that came back `None` would make that guard a silent
/// no-op and leave a file behind for the next `serve` to sweep, which is exactly what nobody
/// would notice.
#[test]
fn an_idle_server_exits_clean_and_takes_its_socket_with_it() {
    let dir = repo_with_docs();
    let mut server = serve(dir.path(), &["--every", "3600", "--idle", "1"]);
    let start = Instant::now();
    let status = loop {
        if let Some(status) = server.try_wait().unwrap() { break status; }
        assert!(start.elapsed() < Duration::from_secs(10), "a server idle for a second is still running");
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut err = String::new();
    std::io::Read::read_to_string(&mut server.stderr.take().unwrap(), &mut err).unwrap();
    assert!(status.success(), "{status}: {err}");
    // Printed after the bind, so the file asserted gone below is one this process really made:
    // a server that never bound would leave no socket either, and prove nothing by it.
    assert!(err.contains("idle 1s, model idle 300s; Ctrl-C stops"), "the server never bound: {err}");
    assert!(err.contains("idle for 1s"), "it left for some other reason: {err}");
    assert!(std::fs::symlink_metadata(socket_of(dir.path())).is_err(), "the socket file outlived its server: {err}");
}

/// The two ways to ask for a cold answer while a server is up. `--no-serve` is a flag on the
/// command line; `ZEROGRAPH_NO_SERVE` is what a benchmark or a wrapper sets, and nothing tested
/// it anywhere. Both have to reach the same answer without the socket — set on the child rather
/// than on this process, which the tests share and run in at once.
#[test]
fn no_serve_and_its_environment_variable_both_answer_here_under_a_live_server() {
    let dir = repo_with_docs();
    let mut server = serve(dir.path(), &["--every", "3600", "--idle", "60"]);
    let (want, _) = ask_until_resident(dir.path(), &["штраф"]);
    let (flagged, err) = ask(dir.path(), &["--no-serve"], &["штраф"]);
    assert!(!err.contains("serve:"), "--no-serve went to the socket anyway: {err}");
    assert_eq!(flagged, want);
    let out = zerograph().env("ZEROGRAPH_NO_SERVE", "1").args(LEXICAL).arg("--repo").arg(dir.path())
        .args(["ask", "штраф"]).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{err}");
    assert!(!err.contains("serve:"), "ZEROGRAPH_NO_SERVE went to the socket anyway: {err}");
    assert_eq!(String::from_utf8(out.stdout).unwrap(), want);
    server.kill().unwrap();
    let _ = server.wait();
}

/// The dense arm end to end: a store built with vectors, a fused answer in this process, and
/// the same answer from a resident server — on the small model, in whatever cache the environment
/// names. Ignored by default because it wants 470 MB on disk; the Windows CI job runs it with the
/// cache restored between runs, and it is the one place the ONNX Runtime build the Windows
/// binary links opens a session and embeds.
#[test]
#[ignore = "needs the small model in FASTEMBED_CACHE_DIR or ~/.cache/zerograph/fastembed; the Windows CI job runs it"]
fn a_fused_answer_is_resident_on_the_small_model() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    std::fs::write(dir.path().join("docs/pay.md"), "**FR-PAY-1 · MUST · Штраф за отмену**\n\nШтраф списывается сам (INV-1).\n\n**INV-1 · MUST · Деньги не сгорают**\n\nОтмена не сжигает деньги.\n").unwrap();
    std::fs::write(dir.path().join("docs/cal.md"), "**FR-CAL-1 · MUST · Перенос визита**\n\nПеренос не считается отменой.\n").unwrap();
    std::fs::write(dir.path().join("zerograph.toml"), "id_families = [\"FR-PAY\", \"FR-CAL\", \"INV\"]\nembed_model = \"intfloat/multilingual-e5-small\"\n").unwrap();
    let out = zerograph().arg("--repo").arg(dir.path()).arg("build").output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(!err.contains("dense: model unavailable"), "the model did not open: {err}");
    assert!(std::fs::metadata(dir.path().join(".zerograph/vectors.f32")).map(|m| m.len() > 0).unwrap_or(false), "no vectors were written");
    let (want, err) = ask_in(FUSED, dir.path(), &["--no-serve"], &["штраф"]);
    assert!(want.contains("FR-PAY-1"), "{want}");
    assert!(!err.contains("dense: model unavailable"), "{err}");
    let mut server = serve_in(FUSED, dir.path(), &["--every", "3600", "--idle", "120"]);
    let start = Instant::now();
    let (through, _) = loop {
        let (out, err) = ask_in(FUSED, dir.path(), &[], &["штраф"]);
        if err.contains("serve: answered by the resident process") { break (out, err); }
        assert!(start.elapsed() < Duration::from_secs(120), "the fused server never answered over the socket: {err}");
        std::thread::sleep(Duration::from_millis(250));
    };
    assert_eq!(through, want, "the resident fused answer is the process's fused answer");
    // The number the whole exercise is for, on the record in the log: eleven resident asks, the
    // median, against whatever the unix run of this test prints.
    let mut times: Vec<u128> = (0..11).map(|_| { let t = Instant::now(); ask_in(FUSED, dir.path(), &[], &["штраф"]); t.elapsed().as_millis() }).collect();
    times.sort();
    eprintln!("resident fused ask, median of 11: {} ms", times[5]);
    server.kill().unwrap();
    let _ = server.wait();
}

fn stop(dir: &std::path::Path) -> std::process::Output {
    zerograph().arg("--repo").arg(dir).arg("stop").output().unwrap()
}

#[test]
fn stop_ends_a_running_server_through_the_exit_that_removes_its_socket() {
    let dir = repo_with_docs();
    let mut server = serve(dir.path(), &["--every", "3600", "--idle", "600"]);
    ask_until_resident(dir.path(), &["штраф"]);
    let out = stop(dir.path());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("serve has stopped"), "{err}");
    let start = Instant::now();
    let status = loop {
        if let Some(status) = server.try_wait().unwrap() { break status; }
        assert!(start.elapsed() < Duration::from_secs(10), "the server outlived its stop");
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut log = String::new();
    std::io::Read::read_to_string(&mut server.stderr.take().unwrap(), &mut log).unwrap();
    assert!(status.success(), "{status}: {log}");
    assert!(log.contains("terminated, exiting"), "it left for some other reason: {log}");
    assert!(std::fs::symlink_metadata(socket_of(dir.path())).is_err(), "the socket file outlived its server: {log}");
}

#[test]
fn stop_with_no_server_says_so_and_succeeds() {
    let dir = repo_with_docs();
    let out = stop(dir.path());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("no serve answers here"), "{err}");
}

/// The command returns while the server it started keeps answering, which is the whole of
/// `--detach`: no shell job, no `Start-Process`, and the next question already resident.
#[test]
fn a_detached_server_answers_after_the_command_returns_and_stop_ends_it() {
    let dir = repo_with_docs();
    let out = zerograph().args(LEXICAL).arg("--repo").arg(dir.path()).args(["serve", "--detach", "--every", "3600", "--idle", "600"]).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("started in the background"), "{err}");
    let (_, asked) = ask(dir.path(), &[], &["штраф"]);
    assert!(asked.contains("serve: answered by the resident process"), "{asked}");
    assert!(stop(dir.path()).status.success());
    let log = std::fs::read_to_string(dir.path().join(".zerograph/serve.log")).unwrap();
    assert!(log.contains("terminated, exiting"), "{log}");
    assert!(std::fs::symlink_metadata(socket_of(dir.path())).is_err(), "{log}");
}

/// A 0.6.4 `serve` as `stop` meets one: bound at `.zerograph/serve.sock` inside the checkout,
/// where that release put its socket, and leaving the way it did on the stop line — the
/// confirmation, then the socket file gone with its exit. The first line it is sent comes back
/// on the channel.
fn serve_0_6_4(dir: &std::path::Path) -> std::sync::mpsc::Receiver<String> {
    let sock = dir.join(".zerograph/serve.sock");
    assert!(sock.as_os_str().len() <= 100, "0.6.4 would have bound {} under the temp directory", sock.display());
    let listener = transport::bind(&sock);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let s = transport::accept(&listener);
        let mut w = &s;
        let _ = writeln!(w, "{{\"ack\":\"0.6.4\"}}");
        let mut line = String::new();
        let _ = BufReader::new(&s).read_line(&mut line);
        let _ = writeln!(w, "{{\"stopped\":\"0.6.4\"}}");
        // The exit takes a moment after the confirmation, as a real one does, so `stop` has to
        // wait for the file rather than find it already gone.
        std::thread::sleep(Duration::from_millis(300));
        let _ = std::fs::remove_file(&sock);
        let _ = tx.send(line);
    });
    rx
}

#[test]
fn stop_ends_a_0_6_4_server_still_bound_inside_the_checkout() {
    let dir = repo_with_docs();
    let sent = serve_0_6_4(dir.path());
    let out = stop(dir.path());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("serve has stopped"), "{err}");
    assert_eq!(sent.recv_timeout(Duration::from_secs(10)).unwrap().trim(), r#"{"stop":true}"#);
    assert!(std::fs::symlink_metadata(dir.path().join(".zerograph/serve.sock")).is_err());
}

/// The old server polls and writes without the writer lock, so a new one that started beside it
/// would be two writers on one store.
#[test]
fn serve_stops_a_0_6_4_server_of_its_checkout_before_it_binds() {
    let dir = repo_with_docs();
    let sent = serve_0_6_4(dir.path());
    let _server = serve(dir.path(), &["--every", "3600", "--idle", "60"]);
    assert_eq!(sent.recv_timeout(Duration::from_secs(20)).expect("serve never sent the old server the stop line").trim(), r#"{"stop":true}"#);
    ask_until_resident(dir.path(), &["штраф"]);
}
