//! A store an earlier release wrote, and the one walk that brings it up to this build's grammar.
//! The store's files are all current by hash, so nothing but the stamp on the manifest can say
//! that a re-read would find more than is in it.

use std::process::Command;

fn zerograph(repo: &std::path::Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_zerograph"))
        .env_remove("ZEROGRAPH_CODE_GLOBS")
        .arg("--no-dense").arg("--repo").arg(repo).args(args).output().unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// Read as text, not bytes: a failure on one of these is a diff a person has to read.
fn store_file(repo: &std::path::Path, name: &str) -> String {
    std::fs::read_to_string(repo.join(".zerograph").join(name)).unwrap()
}

/// The store as a release before the stamp left it: a manifest that names no grammar, and a graph
/// holding none of the citations that release's reader never looked for. Both halves matter — the
/// stamp is what a writer reads, and the empty `pending` is the symptom a user sees.
fn age_the_store(repo: &std::path::Path) {
    let edit = |name: &str, key: &str, value: serde_json::Value| {
        let p = repo.join(".zerograph").join(name);
        let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        v[key] = value;
        std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
    };
    edit("graph.json", "pending", serde_json::json!([]));
    edit("manifest.json", "grammar", serde_json::json!(0));
}

const HELD: &str = "held aside: 1 edges to ids in 1 prefixes no line defines  ISO";

#[test]
fn a_store_from_before_the_stamp_is_re_read_once_and_the_walk_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    std::fs::create_dir_all(repo.join("docs")).unwrap();
    std::fs::write(repo.join("docs/req.md"), "**REQ-7 · MUST · Отмена визита**\n\nсроки по ISO-8601\n").unwrap();
    assert!(zerograph(repo, &["build"]).0);
    let (ok, out, err) = zerograph(repo, &["verify"]);
    assert!(ok && out.contains(HELD), "{out}{err}");

    age_the_store(repo);
    let (ok, out, _) = zerograph(repo, &["verify"]);
    assert!(ok && out.contains("held aside: 0 edges"), "the state the stamp exists to end: {out}");

    let (ok, out, err) = zerograph(repo, &["update"]);
    assert!(ok, "{out}{err}");
    assert!(out.starts_with("changed 0 removed 0"), "no file moved, so the diff names none: {out}");
    assert!(err.contains("grammar:") && err.contains("re-reading all 1 files once"), "{err}");
    let (ok, out, _) = zerograph(repo, &["verify"]);
    assert!(ok && out.contains(HELD), "the citation the older grammar never looked for: {out}");

    // Once. The next update says nothing about the grammar and leaves the store to the byte.
    let healed = store_file(repo, "graph.json");
    let (ok, out, err) = zerograph(repo, &["update"]);
    assert!(ok && !err.contains("grammar:"), "{out}{err}");
    assert_eq!(store_file(repo, "graph.json"), healed);
}

/// The walk belongs to the paths that may write. `ask` is one of them — a store that under-reads
/// its own corpus while answering questions is the whole of the harm — and `--stale`, which
/// promises the store as it stands, is not, however far behind that store is.
#[test]
fn a_reader_that_refreshes_heals_the_store_and_a_stale_one_leaves_it_alone() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    std::fs::create_dir_all(repo.join("docs")).unwrap();
    std::fs::write(repo.join("docs/req.md"), "**REQ-7 · MUST · Отмена визита**\n\nсроки по ISO-8601\n").unwrap();
    assert!(zerograph(repo, &["build"]).0);
    age_the_store(repo);
    let aged = (store_file(repo, "graph.json"), store_file(repo, "manifest.json"));

    let (ok, out, err) = zerograph(repo, &["ask", "--stale", "REQ-7"]);
    assert!(ok, "{out}{err}");
    assert!(!err.contains("grammar:"), "{err}");
    assert_eq!((store_file(repo, "graph.json"), store_file(repo, "manifest.json")), aged);

    let (ok, out, err) = zerograph(repo, &["ask", "REQ-7"]);
    assert!(ok, "{out}{err}");
    assert!(err.contains("grammar:"), "{err}");
    let (ok, out, _) = zerograph(repo, &["verify"]);
    assert!(ok && out.contains(HELD), "{out}");
}

/// A store stamped by the generation before this build's is re-read once by the first writer after
/// an upgrade, which says so: 0.5.x and the forward-ported `main` stamped 4 at most.
#[test]
fn a_store_0_5_x_wrote_is_re_read_once() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    std::fs::create_dir_all(repo.join("docs")).unwrap();
    std::fs::write(repo.join("docs/req.md"), "**REQ-7 · MUST · Отмена визита**\n\nсроки по ISO-8601\n").unwrap();
    assert!(zerograph(repo, &["build"]).0);
    let p = repo.join(".zerograph").join("manifest.json");
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    let older = v["grammar"].as_u64().unwrap() - 1;
    v["grammar"] = serde_json::json!(older);
    std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
    let (ok, out, err) = zerograph(repo, &["update"]);
    assert!(ok, "{out}{err}");
    let notice = format!("generation {older} and this build reads by {}", older + 1);
    assert!(err.contains(&notice) && err.contains("re-reading all 1 files once"), "{err}");
    let (ok, _, err) = zerograph(repo, &["update"]);
    assert!(ok && !err.contains("grammar:"), "{err}");
}
