//! A day with the binary, on the path a person keeps a project under: a repository directory that
//! is Cyrillic and holds a space, a `zerograph.toml` written the way an editor on Windows writes
//! one, a build, a resident `serve` and a question answered through its socket, `ask --rerank` through
//! the configured shell against a stand-in for `claude`, an edit, `update`, `changes`, and finally
//! the server killed the way a person kills one — leaving a socket file for the next `serve` to
//! sweep. Every one of those is exercised somewhere on its own; none of them together, and none
//! under a path that is not ASCII.
//!
//! Written as a test rather than as a shell script on purpose: the three platforms this runs on
//! then run the same code, instead of three scripts that have to be kept saying the same thing.
//! What genuinely differs is the README's PowerShell recipe for starting a background server,
//! which is a claim about PowerShell and is checked on Windows, in the job, where it means
//! something.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Cyrillic and a space, and ASCII file names underneath it: git's default `core.quotepath`
/// escapes a non-ASCII *file* name in its diff headers, which is a finding of its own and not
/// this test's subject. The directory is what the socket, the walk and `git -C` are judged on.
const REPO_DIR: &str = "Проект beauty crm";

/// A home of the tests' own, shared by every child of this binary so a client and its server agree
/// on it: the socket lives under the user's cache, and a server a failed or killed test leaves
/// behind would otherwise leave its file in the real one. Under `/tmp` on unix because a macOS
/// temp directory is deep enough to push the socket past `sun_path` into the fallback.
fn test_home() -> &'static std::path::Path {
    static HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    HOME.get_or_init(|| if cfg!(unix) { tempfile::tempdir_in("/tmp") } else { tempfile::tempdir() }.unwrap()).path()
}

fn zerograph() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_zerograph"));
    c.env_remove("ZEROGRAPH_CODE_GLOBS").env("HOME", test_home()).env("USERPROFILE", test_home());
    c
}

/// Where the server for `repo` listens, in the binary's own words: `status --json` names the socket
/// it checks, which is the one `serve` binds. The name is a hash of the checkout path, kept under
/// the user's cache, so this Cyrillic path with its space is what the hash is taken of and no
/// longer what has to fit in `sun_path`.
fn socket_of(repo: &Path) -> PathBuf {
    let status = run(repo, &["status", "--json"], None);
    let v: serde_json::Value = serde_json::from_str(&status.out).unwrap();
    PathBuf::from(v["serve"]["socket"].as_str().unwrap_or_else(|| panic!("status names no socket: {v}")))
}

/// `git` with an identity of its own and the machine's configuration kept out of it: a runner has
/// no `user.name`, a developer has a signing key and hooks, and all this repository has to do is
/// hold one commit for `changes --base HEAD` to diff against.
fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git").arg("-C").arg(repo)
        .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR").env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .args(["-c", "user.name=zerograph tests", "-c", "user.email=tests@example.invalid", "-c", "commit.gpgsign=false"])
        .args(args).output().unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

struct Run { out: String, err: String }

fn run(repo: &Path, args: &[&str], shell_path: Option<&Path>) -> Run {
    let mut cmd = zerograph();
    cmd.arg("--no-dense").arg("--repo").arg(repo).args(args);
    // On the child, never on this process: tests share one environment and run at once.
    if let Some(dir) = shell_path { cmd.env("PATH", stand_in_path(dir)); }
    let out = cmd.output().unwrap();
    let r = Run {
        out: String::from_utf8_lossy(&out.stdout).into_owned(),
        err: String::from_utf8_lossy(&out.stderr).into_owned(),
    };
    assert!(out.status.success(), "zerograph {args:?} exited {}: {}{}", out.status, r.out, r.err);
    r
}

/// The PATH the reranking child gets: the stand-in `claude` first, and on unix only the two
/// directories a shell and `awk` live in after it. A developer machine has a real `claude` on its
/// PATH, and an inherited one would let it answer — slowly, with that person's tokens, and green.
/// Windows keeps the rest of the PATH: nothing there is a `claude`, and the shell resolver reads
/// it to find Git's `sh`. What makes a real answer red on either is the assertion on the text.
fn stand_in_path(bin: &Path) -> std::ffi::OsString {
    let mut dirs = vec![bin.to_path_buf()];
    #[cfg(unix)]
    dirs.extend([PathBuf::from("/bin"), PathBuf::from("/usr/bin")]);
    #[cfg(windows)]
    {
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        dirs.extend(std::env::split_paths(&inherited));
    }
    std::env::join_paths(dirs).unwrap()
}

/// A shell script named the way the default `rerank_command` names it — no extension, a `#!`
/// line — because that is the shape npm's own shim for a node CLI has, and the reason the
/// configured command runs under `sh` on every platform rather than under `cmd`.
fn fake_claude(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let claude = dir.join("claude");
    // Picks every candidate it was shown, last first, and leaves a mark beside itself: the mark is
    // what tells this stand-in's answer from a real `claude` that outlived the PATH below.
    std::fs::write(&claude, "#!/bin/sh\nawk -F'\\t' 'NF>1{ids[n++]=$1} END{for(i=n-1;i>=0;i--)print ids[i]}'\n: > \"$(dirname \"$0\")/ran\"\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn a_repository_under_a_cyrillic_directory_with_a_space_is_built_served_asked_reranked_and_diffed() {
    let root = tempfile::Builder::new().prefix("rg").tempdir().unwrap();
    let repo = root.path().join(REPO_DIR);
    std::fs::create_dir_all(repo.join("docs")).unwrap();
    std::fs::write(repo.join("docs/pay.md"), "**FR-PAY-1 · MUST · Штраф за отмену**\n\nШтраф списывается сам (INV-1).\n\n**INV-1 · MUST · Деньги не сгорают**\n\nОтмена не сжигает деньги.\n").unwrap();
    std::fs::write(repo.join("docs/cal.md"), "**FR-CAL-1 · MUST · Перенос визита**\n\nПеренос не считается отменой.\n").unwrap();
    std::fs::write(repo.join("README.md"), "# Проект\n\nЗаписи и оплаты.\n").unwrap();
    // What the README tells a user to write, so the store this test makes is not also a file the
    // diff below has to explain.
    std::fs::write(repo.join(".gitignore"), ".zerograph/\n").unwrap();
    // A byte-order mark and CRLF endings: the bytes Notepad and Visual Studio write, and the
    // bytes a file authored on Windows still has when a colleague reads it on Linux. toml_parser
    // drops the mark in its lexer, which is why this is expected to parse rather than to fail.
    std::fs::write(repo.join("zerograph.toml"), "\u{feff}id_families = [\"FR-PAY\", \"FR-CAL\", \"INV\"]\r\n").unwrap();
    git(&repo, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "--no-verify", "-m", "the project as it stands"]);

    let built = run(&repo, &["build"], None);
    let nodes = built.out.split_whitespace().skip_while(|w| *w != "nodes").nth(1).unwrap_or("0");
    assert!(nodes.parse::<u32>().unwrap_or(0) > 0, "the walk found nothing under this path: {}{}", built.out, built.err);

    let mut server = zerograph().arg("--no-dense").arg("--repo").arg(&repo)
        .args(["serve", "--every", "3600", "--idle", "120"])
        .stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    let direct = run(&repo, &["ask", "--no-serve", "штраф"], None).out;
    assert!(direct.contains("FR-PAY-1"), "{direct}");
    let start = Instant::now();
    let resident = loop {
        let r = run(&repo, &["ask", "штраф"], None);
        if r.err.contains("serve: answered by the resident process") { break r; }
        assert!(start.elapsed() < Duration::from_secs(30), "serve never answered on its socket under this path: {}", r.err);
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(resident.out, direct, "the resident answer is the process's answer");

    // The line a user's `ask --rerank` really runs: the configured default, `claude` on PATH,
    // through whichever `sh` this platform resolves — Git for Windows' where std has no shell of
    // its own. Nothing else spawns the default command; the other tests hand `run_command` an awk
    // line. `--no-serve`, because the resident process kept the PATH it started with.
    let bin = root.path().join("bin");
    fake_claude(&bin);
    let reranked = run(&repo, &["ask", "--no-serve", "--rerank", "штраф"], Some(&bin));
    assert!(!reranked.err.contains("rerank:"), "the default rerank command failed: {}", reranked.err);
    assert!(reranked.out.contains("FR-PAY-1"), "{}", reranked.out);
    assert!(bin.join("ran").exists(), "something other than the stand-in answered: {}{}", reranked.out, reranked.err);

    // An edit under a live server, then the two commands that read git from this path.
    let mut readme = std::fs::read_to_string(repo.join("README.md")).unwrap();
    readme.push_str("\nОтмена визита оплачивается.\n");
    std::fs::write(repo.join("README.md"), readme).unwrap();
    let updated = run(&repo, &["update"], None);
    assert!(updated.out.starts_with("changed 1 "), "one file changed: {}", updated.out);
    let changes = run(&repo, &["changes", "--base", "HEAD"], None);
    assert!(changes.out.contains("README.md"), "`git -C` under this path found no diff: {}{}", changes.out, changes.err);

    // Killed, which is what `Stop-Process` and a closed terminal both are: no exit runs, so the
    // socket file stays. The README says so, and the next `serve` is what sweeps it.
    let sock = socket_of(&repo);
    server.kill().unwrap();
    let _ = server.wait();
    assert!(std::fs::symlink_metadata(&sock).is_ok(), "a killed server left no socket to sweep");
    let swept = zerograph().arg("--no-dense").arg("--repo").arg(&repo).args(["serve", "--idle", "1"]).output().unwrap();
    let err = String::from_utf8_lossy(&swept.stderr).into_owned();
    assert!(swept.status.success(), "the next serve would not bind over the dead socket: {err}");
    assert!(err.contains("idle for 1s"), "{err}");
    assert!(std::fs::symlink_metadata(&sock).is_err(), "and took its own socket with it: {err}");
}
