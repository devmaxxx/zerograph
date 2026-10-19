//! A reader never waits on a refresh longer than its budget: past it, the answer comes from the
//! store as it stands, one line says so, `--json` names what it was given without, and one
//! detached `update` catches the store up for the next reader.

mod common;

use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

fn reader(repo: &Path, budget: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zerograph"))
        .env_remove("ZEROGRAPH_BENCH_REPO")
        .env_remove("ZEROGRAPH_EMBED_MODEL")
        .env_remove("ZEROGRAPH_CODE_GLOBS")
        .env_remove("ZEROGRAPH_TEXT_GLOBS")
        .env("ZEROGRAPH_READER_BUDGET", budget)
        .arg("--no-dense")
        .arg("--repo")
        .arg(repo)
        .args(args)
        .output()
        .unwrap()
}

fn json(out: &Output) -> serde_json::Value {
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("not JSON: {e}\n{text}"))
}

const CODE: &str = "export function refund(id: string) {\n  return write(id);\n}\n\n\
    export function write(id: string) {\n  return id;\n}\n";

const AUDIT: &str = "import { write } from './billing';\n\nexport function audit(id: string) {\n  return write(id);\n}\n";

fn built() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    common::write(dir.path(), "billing.ts", CODE);
    common::ok(dir.path(), &["build"]);
    dir
}

fn lock_file(repo: &Path) -> std::fs::File {
    std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(repo.join(".zerograph/writer.lock")).unwrap()
}

/// Waits until no writer holds the store's lock, so the temporary directory is not removed from
/// under a detached refresh — which Windows refuses outright.
fn settle(repo: &Path) {
    let f = lock_file(repo);
    let start = Instant::now();
    while f.try_lock().is_err() {
        assert!(start.elapsed() < Duration::from_secs(60), "a detached refresh is still holding the lock after a minute");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The callers an impact answer found, without the `stale` field that names the files it lacks.
fn callers(v: &serde_json::Value) -> String { v["layers"].to_string() }

#[test]
fn over_budget_the_stored_graph_answers_and_a_detached_update_catches_it_up() {
    let dir = built();
    let repo = dir.path();
    common::write(repo, "audit.ts", AUDIT);

    let out = reader(repo, "0", &["impact", "write", "--json"]);
    let v = json(&out);
    assert_eq!(v["stale"]["files"], serde_json::json!(["audit.ts"]), "{v}");
    assert_eq!(v["stale"]["vectors"], 0);
    assert!(!callers(&v).contains("audit"), "the answer is the stored graph's: {v}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("index: 1 file behind, refreshing in background"), "{stderr}");

    // The next reader with the default budget either finds the refresh running — and answers
    // stale without starting another — or finds it done.
    let start = Instant::now();
    let fresh = loop {
        let v = json(&reader(repo, "10", &["impact", "write", "--json", "--stale"]));
        if callers(&v).contains("audit") { break v; }
        assert!(start.elapsed() < Duration::from_secs(60), "the detached update never landed: {v}");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(fresh.get("stale").is_none(), "an answer that is not behind carries no `stale`: {fresh}");
    settle(repo);
    let log = std::fs::read_to_string(repo.join(".zerograph/background.log")).unwrap();
    assert!(log.contains("changed 1 removed 0"), "the detached update wrote its report to the log: {log}");
}

#[test]
fn a_reader_that_finds_the_lock_held_answers_stale_and_starts_nothing() {
    let dir = built();
    let repo = dir.path();
    common::write(repo, "audit.ts", AUDIT);
    let held = lock_file(repo);
    held.lock().unwrap();

    let out = reader(repo, "10", &["impact", "write", "--json"]);
    let v = json(&out);
    assert_eq!(v["stale"]["files"], serde_json::json!(["audit.ts"]), "{v}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("index: 1 file behind, a refresh is already running"), "{stderr}");
    assert!(!repo.join(".zerograph/background.log").exists(), "no refresh was started");

    drop(held);
    let v = json(&reader(repo, "10", &["impact", "write", "--json"]));
    assert!(v.get("stale").is_none() && callers(&v).contains("audit"), "with the lock free it refreshes inline: {v}");
}

#[test]
fn text_answers_carry_no_stale_field_and_ask_says_the_same_line() {
    let dir = built();
    let repo = dir.path();
    common::write(repo, "audit.ts", AUDIT);
    let out = reader(repo, "0", &["ask", "--no-serve", "write"]);
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("index: 1 file behind"), "{stderr}");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("\"stale\""));
    let v = json(&reader(repo, "0", &["ask", "--no-serve", "--json", "write"]));
    // Started by the first reader or this one, the refresh may already have landed.
    if let Some(s) = v.get("stale") { assert_eq!(s["files"], serde_json::json!(["audit.ts"]), "{v}"); }
    settle(repo);
}

#[test]
fn update_detach_returns_at_once_and_the_store_catches_up() {
    let dir = built();
    let repo = dir.path();
    common::write(repo, "audit.ts", AUDIT);
    let out = common::run(repo, &["update", "--detach"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.stdout.is_empty(), "the report goes to the log, not here");
    let start = Instant::now();
    while !callers(&json(&reader(repo, "10", &["impact", "write", "--json", "--stale"]))).contains("audit") {
        assert!(start.elapsed() < Duration::from_secs(60), "the detached update never landed");
        std::thread::sleep(Duration::from_millis(100));
    }
    settle(repo);
}
