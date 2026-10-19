//! The binary over a Swift repository: inheritance across files, and a body-only edit.

use std::path::Path;
use std::process::Command;

/// The binary as `tests/common` spawns it, with the two bench variables removed for the same reason,
/// and Swift globbed through the environment so the repository needs no `zerograph.toml`.
fn zerograph(repo: &Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_zerograph"))
        .env_remove("ZEROGRAPH_BENCH_REPO")
        .env_remove("ZEROGRAPH_EMBED_MODEL")
        .env("ZEROGRAPH_CODE_GLOBS", "**/*.swift")
        .arg("--no-dense")
        .arg("--repo")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

fn write(repo: &Path, rel: &str, text: &str) {
    let p = repo.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[test]
fn impact_on_a_protocol_names_a_conforming_class_and_an_extension_elsewhere() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "Sources/Store.swift", "protocol Store {\n    func load() -> Int\n}\n");
    write(repo, "Sources/Cart.swift", "final class Cart: Store {\n    func load() -> Int { 1 }\n}\n");
    write(repo, "Sources/Point.swift", "struct Point {}\n\nextension Point: Store {\n    func load() -> Int { 2 }\n}\n");
    let (ok, out, err) = zerograph(repo, &["build"]);
    assert!(ok, "{out}{err}");
    let (ok, out, err) = zerograph(repo, &["impact", "Store"]);
    assert!(ok, "{err}");
    assert!(out.contains("Sources/Cart.swift") && out.contains("Sources/Point.swift"), "{out}");
}

/// Swift has no `header_for` arm, so the edge to a superclass added in another file is written when the
/// subclass is read. Only that read is asserted: whether an update reaches it sooner is the master's rule.
#[test]
fn a_superclass_added_later_is_an_edge_once_the_subclass_is_read_again() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "Sources/Handler.swift", "class Handler: Base {\n    func run() {}\n}\n");
    let (ok, out, err) = zerograph(repo, &["build"]);
    assert!(ok, "{out}{err}");
    write(repo, "Sources/Base.swift", "class Base {}\n");
    write(repo, "Sources/Handler.swift", "class Handler: Base {\n    func run() { }\n}\n");
    let (ok, _, err) = zerograph(repo, &["update"]);
    assert!(ok, "{err}");
    let (ok, out, err) = zerograph(repo, &["impact", "Base"]);
    assert!(ok, "{err}");
    assert!(out.contains("Sources/Handler.swift"), "{out}");
}

#[test]
fn a_body_only_edit_re_reads_that_file_alone() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "Sources/A.swift", "class A {\n    func f() -> Int { 1 }\n}\n");
    write(repo, "Sources/B.swift", "class B: A {}\n");
    let (ok, out, err) = zerograph(repo, &["build"]);
    assert!(ok, "{out}{err}");
    write(repo, "Sources/A.swift", "class A {\n    func f() -> Int { 2 }\n}\n");
    let (ok, out, err) = zerograph(repo, &["update"]);
    assert!(ok, "{err}");
    assert!(out.starts_with("changed 1 removed 0"), "{out}");
}
