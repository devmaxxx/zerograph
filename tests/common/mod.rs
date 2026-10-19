//! How the tests that include this module spawn the binary. Not every suite does — `serve.rs` and
//! `users_day.rs` spawn their own, because what they measure is a binary at a path of their own.
//!
//! The child's environment is the parent's less the two variables the bench kit exports, because
//! each of them redirects what a command reads. `ZEROGRAPH_BENCH_REPO` moves `bench` off the
//! `--repo` it was given — with it exported, the floors-missed case reads another corpus, finds no
//! graph and exits 1 where the suite is pinning a 3 — and `ZEROGRAPH_EMBED_MODEL` outranks the
//! model a store records, so a shell left over from a measurement answers these tests under other
//! weights. Both are invisible in the failure they cause, which is why they are stripped here
//! rather than documented somewhere a reader would have to find. `ZEROGRAPH_CODE_GLOBS` and
//! `ZEROGRAPH_TEXT_GLOBS` replace the globs a test's own `zerograph.toml` names.

// Compiled into a test crate per file that declares `mod common`, each of which uses the part of
// it that it needs; anything the others use is dead code from where that one stands.
#![allow(dead_code)]

pub mod code;

use std::path::Path;
use std::process::{Command, Output};

pub fn run(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zerograph"))
        .env_remove("ZEROGRAPH_BENCH_REPO")
        .env_remove("ZEROGRAPH_EMBED_MODEL")
        .env_remove("ZEROGRAPH_CODE_GLOBS")
        .env_remove("ZEROGRAPH_TEXT_GLOBS")
        .env_remove("ZEROGRAPH_READER_BUDGET")
        .arg("--no-dense")
        .arg("--repo")
        .arg(repo)
        .args(args)
        .output()
        .unwrap()
}

/// A hook exports its own repository into what it runs, and those variables outrank `-C`: without
/// stripping them a fixture's `git add -A` and `git commit` land in the repository being committed.
pub fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

pub fn write(repo: &Path, rel: &str, text: &str) {
    let path = repo.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Runs a command that must succeed and returns its stdout.
pub fn ok(repo: &Path, args: &[&str]) -> String {
    let out = run(repo, args);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{args:?}: {stdout}{}", String::from_utf8_lossy(&out.stderr));
    stdout
}

/// The node ids of a `trace` answer, one per line.
pub fn path(out: &str) -> Vec<&str> {
    out.lines().map(|l| l.trim_start_matches("  → ").split_whitespace().next().unwrap_or("")).collect()
}
