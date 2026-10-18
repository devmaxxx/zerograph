mod common;

use common::{ok, run, write};

fn built() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "docs/pay.md", "**FR-PAY-1 · MUST · cancellation window**\n\nA booking may be cancelled up to a day before.\n");
    write(dir.path(), "zerograph.toml", "id_families = [\"FR-PAY\"]\n");
    ok(dir.path(), &["build"]);
    dir
}

#[test]
fn status_reports_the_index_the_tree_and_that_nothing_runs() {
    let dir = built();
    let text = ok(dir.path(), &["status"]);
    assert!(text.contains("doc nodes") && text.contains("tree: the index is in step"), "{text}");
    assert!(text.contains("refresh: none running") && text.contains("serve: not running"), "{text}");

    write(dir.path(), "docs/more.md", "# More\n");
    let text = ok(dir.path(), &["status"]);
    assert!(text.contains("1 file behind (docs/more.md)"), "{text}");
}

#[test]
fn status_json_carries_the_same_facts_as_one_object() {
    let dir = built();
    write(dir.path(), ".zerograph/background.log", "refresh: started\nrefresh: 1 changed, 0 removed\nembedded 4/4 (2.0 rows/s, ~0 s left)\n");
    let v: serde_json::Value = serde_json::from_str(&ok(dir.path(), &["status", "--json"])).unwrap();
    assert!(v["index"]["doc_nodes"].as_u64().unwrap() > 0, "{v}");
    assert_eq!(v["tree"]["state"], "in_step");
    assert_eq!(v["refresh"]["state"], "none");
    assert_eq!(v["last_run"], serde_json::json!(["refresh: 1 changed, 0 removed"]));
    assert_eq!(v["serve"]["running"], false);
}

#[test]
fn follow_with_nothing_running_says_so_and_exits_clean() {
    let dir = built();
    let out = run(dir.path(), &["status", "--follow"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).ends_with("refresh: none running\n"));
}

#[test]
fn a_project_never_built_is_reported_empty_and_left_unwritten() {
    let dir = tempfile::tempdir().unwrap();
    let text = ok(dir.path(), &["status"]);
    assert!(text.starts_with("index: empty; `zerograph build` builds it"), "{text}");
    assert!(!dir.path().join(".zerograph").exists(), "status created the store directory");
}

#[cfg(unix)]
#[test]
fn a_detached_serve_is_reported_answering_with_its_log() {
    let dir = built();
    let out = run(dir.path(), &["serve", "--detach", "--every", "3600", "--idle", "600"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_str(&ok(dir.path(), &["status", "--json"])).unwrap();
    let stopped = run(dir.path(), &["stop"]);
    assert_eq!(v["serve"]["running"], true, "{v}");
    assert!(v["serve"]["log"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().ends_with("; Ctrl-C stops")), "{v}");
    assert!(stopped.status.success());
}
