//! The binary over a JVM repository: `update` after a nested type or a member appears, disappears
//! or changes visibility writes the graph a fresh `build` of the same tree writes; and, separately,
//! what `update` re-reads when a declaration appears or disappears (L3), and a call path that
//! crosses Kotlin and Java twice.

mod common;

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

/// Kotlin and Java are read behind an explicit glob until L7 puts them in the defaults. The globs go
/// in the child's environment so no `zerograph.toml` is written.
fn ok(repo: &Path, args: &[&str]) {
    let out = Command::new(env!("CARGO_BIN_EXE_zerograph"))
        .env_remove("ZEROGRAPH_BENCH_REPO")
        .env_remove("ZEROGRAPH_EMBED_MODEL")
        .env("ZEROGRAPH_CODE_GLOBS", "**/*.kt **/*.java")
        .arg("--no-dense")
        .arg("--repo")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
}

fn write(repo: &Path, rel: &str, contents: impl AsRef<[u8]>) {
    let p = repo.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents.as_ref()).unwrap();
}

/// `run`, decoded: whether the binary exited 0, and its stdout and stderr as text.
fn zerograph(repo: &Path, args: &[&str]) -> (bool, String, String) {
    let out = common::run(repo, args);
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// (source, target, kind, context).
type Edges = BTreeSet<(String, String, String, String)>;

fn edges(repo: &Path) -> Edges {
    let text = std::fs::read_to_string(repo.join(".zerograph/graph.json")).unwrap();
    let graph: serde_json::Value = serde_json::from_str(&text).unwrap();
    let field = |e: &serde_json::Value, k: &str| e[k].as_str().unwrap_or_default().to_string();
    graph["edges"].as_array().unwrap().iter().map(|e| (field(e, "source"), field(e, "target"), field(e, "kind"), field(e, "context"))).collect()
}

/// Builds `before`, rewrites it to `after`, runs `update`, and returns that graph's edges with the
/// edges of a fresh build of `after`.
fn updated_and_built(before: &[(&str, &str)], after: &[(&str, &str)]) -> (Edges, Edges) {
    removed_updated_and_built(before, after, &[])
}

/// `updated_and_built`, with the files at `removed` deleted before the update and absent from the
/// fresh build.
fn removed_updated_and_built(before: &[(&str, &str)], after: &[(&str, &str)], removed: &[&str]) -> (Edges, Edges) {
    let old = tempfile::tempdir().unwrap();
    for (rel, text) in before {
        write(old.path(), rel, text);
    }
    ok(old.path(), &["build"]);
    for (rel, text) in after {
        write(old.path(), rel, text);
    }
    for rel in removed {
        std::fs::remove_file(old.path().join(rel)).unwrap();
    }
    ok(old.path(), &["update"]);
    let fresh = tempfile::tempdir().unwrap();
    for (rel, text) in before.iter().chain(after).filter(|(rel, _)| !removed.contains(rel)) {
        write(fresh.path(), rel, text);
    }
    ok(fresh.path(), &["build"]);
    (edges(old.path()), edges(fresh.path()))
}

const USE: (&str, &str) = ("shop/orders/Use.java", "package shop.orders;\n\nimport shop.billing.Invoice.Line;\n\nclass Use extends Line {}\n");
const BARE: (&str, &str) = ("shop/billing/Invoice.java", "package shop.billing;\n\npublic class Invoice {}\n");
const NESTED: (&str, &str) = ("shop/billing/Invoice.java", "package shop.billing;\n\npublic class Invoice {\n    public static class Line {}\n}\n");

#[test]
fn a_nested_type_added_reaches_the_unchanged_file_that_extends_it() {
    let (updated, built) = updated_and_built(&[BARE, USE], &[NESTED]);
    let extends = ("sym:shop/orders/Use.java::Use".to_string(), "sym:shop/billing/Invoice.java::Invoice.Line".to_string(), "Extends".to_string(), String::new());
    assert!(built.contains(&extends), "{built:?}");
    assert_eq!(updated, built);
}

#[test]
fn a_nested_type_removed_leaves_no_edge_to_it_after_update() {
    let (updated, built) = updated_and_built(&[NESTED, USE], &[BARE]);
    assert!(!updated.iter().any(|(_, to, _, _)| to.contains("Invoice.Line")), "{updated:?}");
    assert_eq!(updated, built);
}

#[test]
fn a_kotlin_member_added_reaches_the_file_that_imports_it() {
    let keys = |body: &str| ("shop/Keys.kt", format!("package shop\n\nobject Keys {{\n{body}}}\n"));
    let (rel, before) = keys("");
    let (_, after) = keys("    const val TOKEN = \"t\"\n");
    let use_kt = ("app/Use.kt", "package app\n\nimport shop.Keys.TOKEN\n\nclass Use\n");
    let (updated, built) = updated_and_built(&[(rel, &before), use_kt], &[(rel, &after)]);
    assert!(built.iter().any(|(from, to, kind, _)| from == "file:app/Use.kt" && to == "file:shop/Keys.kt" && kind == "Imports"), "{built:?}");
    assert_eq!(updated, built);
}

#[test]
fn a_java_method_made_public_reaches_the_subclass_in_another_file_that_calls_it() {
    let base = |vis: &str| ("shop/Base.java", format!("package shop;\n\npublic class Base {{\n    {vis} void helper() {{}}\n}}\n"));
    let (rel, before) = base("private");
    let (_, after) = base("public");
    let sub = ("shop/Sub.java", "package shop;\n\nclass Sub extends Base {\n    void go() { helper(); }\n}\n");
    let (updated, built) = updated_and_built(&[(rel, &before), sub], &[(rel, &after)]);
    let call = ("sym:shop/Sub.java::Sub.go".to_string(), "sym:shop/Base.java::Base.helper".to_string(), "Calls".to_string(), String::new());
    assert!(built.contains(&call), "{built:?}");
    assert_eq!(updated, built);
}

#[test]
fn a_java_method_that_comes_to_take_a_call_s_arguments_is_bound_from_the_subclass_in_another_file() {
    let base = |params: &str| ("shop/Base.java", format!("package shop;\n\npublic class Base {{\n    public void helper({params}) {{}}\n}}\n"));
    let (rel, before) = base("int n");
    let (_, after) = base("");
    let sub = ("shop/Sub.java", "package shop;\n\nclass Sub extends Base {\n    void go() { helper(); }\n}\n");
    let (updated, built) = updated_and_built(&[(rel, &before), sub], &[(rel, &after)]);
    let call = ("sym:shop/Sub.java::Sub.go".to_string(), "sym:shop/Base.java::Base.helper".to_string(), "Calls".to_string(), String::new());
    assert!(built.contains(&call), "{built:?}");
    assert_eq!(updated, built);
}

#[test]
fn a_kotlin_member_made_public_reaches_the_subclass_in_another_file_that_calls_it() {
    let base = |vis: &str| ("app/A.kt", format!("package app\n\nopen class A {{\n    {vis} fun helper() {{}}\n}}\n"));
    let (rel, before) = base("private");
    let (_, after) = base("public");
    let sub = ("app/Sub.kt", "package app\n\nclass Sub : A() {\n    fun go() { helper() }\n}\n");
    let call = ("sym:app/Sub.kt::Sub.go".to_string(), "sym:app/A.kt::A.helper".to_string(), "Calls".to_string(), String::new());
    let (updated, built) = updated_and_built(&[(rel, &before), sub], &[(rel, &after)]);
    assert!(built.contains(&call), "{built:?}");
    assert_eq!(updated, built);
    let (updated, built) = updated_and_built(&[(rel, &after), sub], &[(rel, &before)]);
    assert!(!built.contains(&call), "{built:?}");
    assert_eq!(updated, built);
}

#[test]
fn a_kotlin_overload_made_public_beside_a_public_one_reaches_the_subclass_in_another_file() {
    let base = |vis: &str| ("app/A.kt", format!("package app\n\nopen class A {{\n    {vis} fun helper() {{}}\n    fun helper(x: Int) {{}}\n}}\n"));
    let (rel, before) = base("private");
    let (_, after) = base("public");
    let sub = ("app/Sub.kt", "package app\n\nclass Sub : A() {\n    fun go() { helper() }\n}\n");
    let call = ("sym:app/Sub.kt::Sub.go".to_string(), "sym:app/A.kt::A.helper".to_string(), "Calls".to_string(), String::new());
    let (updated, built) = updated_and_built(&[(rel, &before), sub], &[(rel, &after)]);
    assert!(built.contains(&call), "{built:?}");
    assert_eq!(updated, built);
    let (updated, built) = updated_and_built(&[(rel, &after), sub], &[(rel, &before)]);
    assert!(!built.contains(&call), "{built:?}");
    assert_eq!(updated, built);
}

const ANNOTATION: (&str, &str) = ("app/Api.kt", "package app\n\nannotation class Api\n");
const ANNOTATED: (&str, &str) = ("app/Service.kt", "package app\n\n@Api\nclass Service {\n    @Api\n    fun run() {}\n}\n");
const JAVA_ANNOTATED: (&str, &str) = ("app/Job.java", "package app;\n\n@Api\nclass Job {\n    @Api int size;\n}\n");

fn decorated(edges: &Edges) -> Vec<(String, String)> {
    edges.iter().filter(|(_, _, kind, _)| kind == "DecoratedBy").map(|(from, to, _, _)| (from.clone(), to.clone())).collect()
}

#[test]
fn an_annotation_class_added_decorates_the_unchanged_files_that_use_it() {
    let (updated, built) = updated_and_built(&[ANNOTATED, JAVA_ANNOTATED], &[ANNOTATION]);
    assert_eq!(decorated(&built).len(), 4, "{built:?}");
    assert_eq!(updated, built);
}

#[test]
fn an_annotation_class_removed_leaves_no_edge_to_it_after_update() {
    let (updated, built) = removed_updated_and_built(&[ANNOTATION, ANNOTATED, JAVA_ANNOTATED], &[], &[ANNOTATION.0]);
    assert!(decorated(&updated).is_empty(), "{updated:?}");
    assert_eq!(updated, built);
}

#[test]
fn an_annotation_class_renamed_or_moved_is_followed_by_update() {
    let renamed = ("app/Api.kt", "package app\n\nannotation class Endpoint\n");
    let (updated, built) = updated_and_built(&[ANNOTATION, ANNOTATED, JAVA_ANNOTATED], &[renamed]);
    assert!(decorated(&updated).is_empty(), "{updated:?}");
    assert_eq!(updated, built);
    let moved = ("app/meta/Api.kt", "package app\n\nannotation class Api\n");
    let (updated, built) = removed_updated_and_built(&[ANNOTATION, ANNOTATED, JAVA_ANNOTATED], &[moved], &[ANNOTATION.0]);
    assert!(decorated(&updated).iter().all(|(_, to)| to == "sym:app/meta/Api.kt::Api") && decorated(&updated).len() == 4, "{updated:?}");
    assert_eq!(updated, built);
}

/// A JVM file that is not UTF-8. The walk hashes it, and `apply_diff` prints `skipping <rel>: not
/// UTF-8` every time it tries to read it, so that line on an update's stderr is the one outside sign
/// that `widen` put the whole family back into the stale set.
const PROBE: &str = "a/Probe.kt";
const PROBE_BYTES: &[u8] = b"package a\n\n// \xff\n";

fn jvm_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "zerograph.toml", b"code_globs = [\"**/*.kt\", \"**/*.java\"]\n");
    write(repo, "a/A.kt", b"package a\n\nimport shop.Fresh\n\nclass A(private val x: Fresh) {\n    fun f() { x.go() }\n}\n");
    write(repo, "a/B.kt", b"package a\n\nclass B {\n    fun g() = 1\n}\n");
    write(repo, PROBE, PROBE_BYTES);
    dir
}

#[test]
fn a_declaration_added_in_java_reaches_the_unchanged_kotlin_file_that_uses_it() {
    let dir = jvm_repo();
    let repo = dir.path();
    let (ok, _, err) = zerograph(repo, &["build"]);
    // A binary under a JVM glob must not stop the index walk, or the probe could not exist.
    assert!(ok, "{err}");
    write(repo, "shop/Fresh.java", b"package shop;\n\npublic class Fresh {\n    public void go() {}\n}\n");
    let (ok, out, err) = zerograph(repo, &["update"]);
    assert!(ok, "{err}");
    assert!(out.starts_with("changed 1 removed 0"), "only the new file is in the diff: {out}");
    assert!(err.contains(&format!("skipping {PROBE}: not UTF-8")), "an added declaration re-reads the family: {err}");
    let (ok, out, err) = zerograph(repo, &["impact", "Fresh"]);
    assert!(ok, "{err}");
    assert!(out.contains("a/A.kt"), "A.kt was re-read, so it now calls Fresh.go and imports Fresh: {out}");
}

#[test]
fn a_file_removed_from_the_family_widens_too() {
    let dir = jvm_repo();
    let repo = dir.path();
    let (ok, _, err) = zerograph(repo, &["build"]);
    assert!(ok, "{err}");
    std::fs::remove_file(repo.join("a/B.kt")).unwrap();
    let (ok, out, err) = zerograph(repo, &["update"]);
    assert!(ok, "{err}");
    assert!(out.starts_with("changed 0 removed 1"), "{out}");
    assert!(err.contains(&format!("skipping {PROBE}: not UTF-8")), "a removed file re-reads the family: {err}");
}

#[test]
fn a_body_only_edit_re_reads_that_file_alone() {
    let dir = jvm_repo();
    let repo = dir.path();
    let (ok, _, err) = zerograph(repo, &["build"]);
    assert!(ok, "{err}");
    write(repo, "a/B.kt", b"package a\n\nclass B {\n    fun g() = 2\n}\n");
    let (ok, out, err) = zerograph(repo, &["update"]);
    assert!(ok, "{err}");
    assert!(out.starts_with("changed 1 removed 0"), "{out}");
    assert!(!err.contains(PROBE), "a body-only edit does not widen: {err}");
}

#[test]
fn a_call_path_crosses_from_kotlin_to_java_and_back() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "zerograph.toml", b"code_globs = [\"**/*.kt\", \"**/*.java\"]\n");
    write(repo, "app/Start.kt", b"package app\n\nimport shop.Middle\n\nclass Start(private val mid: Middle) {\n    fun run() { mid.pass() }\n}\n");
    write(repo, "shop/Middle.java", b"package shop;\n\nimport app.End;\n\npublic class Middle {\n    private final End end = new End();\n    public void pass() { end.finish(); }\n}\n");
    write(repo, "app/End.kt", b"package app\n\nclass End {\n    fun finish() {}\n}\n");
    let (ok, _, err) = zerograph(repo, &["build"]);
    assert!(ok, "{err}");
    let (ok, out, err) = zerograph(repo, &["trace", "Start", "End"]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("shop/Middle.java"), "the path runs through the Java class: {out}");
    assert!(out.contains("app/End.kt"), "{out}");
}

#[test]
fn a_type_added_or_removed_is_followed_to_the_unchanged_files_whose_signatures_name_it() {
    let key = ("app/Key.kt", "package app\n\nclass Key\n");
    let other = ("app/Other.kt", "package app\n\nclass Other\n");
    let kt = ("app/Driver.kt", "package app\n\ninterface Driver {\n    fun open(key: Key): List<Key>\n}\n");
    let java = ("app/Store.java", "package app;\n\nclass Store {\n    Key key;\n}\n");
    let imports = |edges: &Edges| edges.iter().filter(|(_, to, kind, _)| to == "file:app/Key.kt" && kind == "Imports").count();
    let (updated, built) = updated_and_built(&[other, kt, java], &[key]);
    assert_eq!(imports(&built), 2, "{built:?}");
    assert_eq!(updated, built);
    let (updated, built) = removed_updated_and_built(&[key, other, kt, java], &[], &[key.0]);
    assert_eq!(imports(&built), 0, "{built:?}");
    assert_eq!(updated, built);
}

#[test]
fn a_member_named_like_the_constructor_added_to_a_supertype_in_another_file_drops_the_call_after_update() {
    let wipe = |body: &str| ("app/sync/Wipe.kt", format!("package app.sync\n\ninterface Destroyer {{\n    fun destroy() {{}}\n{body}}}\n"));
    let (rel, plain) = wipe("");
    let (_, shadowing) = wipe("    fun Key(raw: String): Any = raw\n");
    let key = ("app/Key.kt", "package app\n\nclass Key(val raw: String)\n");
    let store = ("app/Store.kt", "package app\n\nimport app.sync.Destroyer\n\nclass Store : Destroyer {\n    fun make(s: String): Any = Key(s)\n}\n");
    let call = ("sym:app/Store.kt::Store.make".to_string(), "sym:app/Key.kt::Key".to_string(), "Calls".to_string(), String::new());
    let (updated, built) = updated_and_built(&[(rel, &plain), key, store], &[(rel, &shadowing)]);
    assert!(!built.contains(&call), "{built:?}");
    assert_eq!(updated, built);
    let (updated, built) = updated_and_built(&[(rel, &shadowing), key, store], &[(rel, &plain)]);
    assert!(built.contains(&call), "{built:?}");
    assert_eq!(updated, built);
}

#[test]
fn a_supertype_s_own_supertype_changed_in_its_file_is_followed_by_update() {
    let mid = |sup: &str| ("app/B.kt", format!("package app\n\nopen class Mid : {sup}()\n"));
    let (rel, walked) = mid("Base");
    let (_, unread) = mid("Exception");
    let base = ("app/A.kt", "package app\n\nopen class Base {\n    fun only() {}\n}\n");
    let sub = ("app/C.kt", "package app\n\nclass Sub : Mid() {\n    fun go() { only() }\n}\n");
    let call = ("sym:app/C.kt::Sub.go".to_string(), "sym:app/A.kt::Base.only".to_string(), "Calls".to_string(), String::new());
    let (updated, built) = updated_and_built(&[(rel, &walked), base, sub], &[(rel, &unread)]);
    assert!(!built.contains(&call), "{built:?}");
    assert_eq!(updated, built);
    let (updated, built) = updated_and_built(&[(rel, &unread), base, sub], &[(rel, &walked)]);
    assert!(built.contains(&call), "{built:?}");
    assert_eq!(updated, built);
}

fn call(from: &str, to: &str) -> (String, String, String, String) {
    (from.to_string(), to.to_string(), "Calls".to_string(), String::new())
}

/// Builds each state, updates it to the other, and checks both ways that `update` writes the
/// graph a fresh build does, with `edge` only in the second state.
fn flipped<'a>(first: (&'a str, &'a str), second: (&'a str, &'a str), rest: &[(&'a str, &'a str)], edge: &(String, String, String, String)) {
    let (updated, built) = updated_and_built(&[&[first][..], rest].concat(), &[second]);
    assert!(built.contains(edge), "{built:?}");
    assert_eq!(updated, built);
    let (updated, built) = updated_and_built(&[&[second][..], rest].concat(), &[first]);
    assert!(!built.contains(edge), "{built:?}");
    assert_eq!(updated, built);
}

#[test]
fn a_superclass_gaining_or_losing_the_member_an_interface_also_declares_is_followed_by_update() {
    let map = ("u/Map.java", "package u;\n\npublic interface Map {\n    boolean isEmpty();\n}\n");
    let hash = ("u/HashMap.java", "package u;\n\npublic class HashMap extends AbstractMap implements Map {\n    void go() { isEmpty(); }\n}\n");
    let plain = "package u;\n\npublic abstract class AbstractMap implements Map {}\n";
    let declaring = "package u;\n\npublic abstract class AbstractMap implements Map {\n    public boolean isEmpty() { return true; }\n}\n";
    let edge = call("sym:u/HashMap.java::HashMap.go", "sym:u/AbstractMap.java::AbstractMap.isEmpty");
    flipped(("u/AbstractMap.java", plain), ("u/AbstractMap.java", declaring), &[map, hash], &edge);
    let sized = ("app/Sized.kt", "package app\n\ninterface Sized {\n    fun isEmpty(): Boolean\n}\n");
    let bag = ("app/Bag.kt", "package app\n\nclass Bag : AbstractSized(), Sized {\n    fun go() { isEmpty() }\n}\n");
    let plain = "package app\n\nabstract class AbstractSized : Sized\n";
    let declaring = "package app\n\nabstract class AbstractSized : Sized {\n    override fun isEmpty() = true\n}\n";
    let edge = call("sym:app/Bag.kt::Bag.go", "sym:app/AbstractSized.kt::AbstractSized.isEmpty");
    flipped(("app/AbstractSized.kt", plain), ("app/AbstractSized.kt", declaring), &[sized, bag], &edge);
}

#[test]
fn an_interface_method_made_static_or_not_is_followed_by_update() {
    let namesake = ("a/u/U.java", "package a.u;\n\npublic class U {\n    public static void help() {}\n}\n");
    let d = ("a/D.java", "package a;\n\nimport static a.u.U.help;\n\nclass D implements J {\n    void go() { help(); }\n}\n");
    let j = ("a/J.java", "package a;\n\npublic interface J extends I {}\n");
    let fixed = "package a;\n\npublic interface I {\n    static void help() {}\n}\n";
    let inherited = "package a;\n\npublic interface I {\n    default void help() {}\n}\n";
    flipped(("a/I.java", fixed), ("a/I.java", inherited), &[namesake, d, j], &call("sym:a/D.java::D.go", "sym:a/I.java::I.help"));
}

#[test]
fn a_method_made_package_private_or_protected_is_followed_by_update() {
    let mid = ("a/Mid.java", "package a;\n\npublic class Mid extends Base {}\n");
    let outer = ("b/Outer.java", "package b;\n\nclass Outer {\n    void help() {}\n    class Deep extends a.Mid {\n        void go() { help(); }\n    }\n}\n");
    let package = "package a;\n\npublic class Base {\n    void help() {}\n}\n";
    let protected = "package a;\n\npublic class Base {\n    protected void help() {}\n}\n";
    flipped(("a/Base.java", package), ("a/Base.java", protected), &[mid, outer], &call("sym:b/Outer.java::Outer.Deep.go", "sym:a/Base.java::Base.help"));
}

#[test]
fn a_record_component_added_or_removed_is_followed_by_update() {
    let h = ("r/H.java", "package r;\n\npublic interface H {\n    default int x() { return -1; }\n}\n");
    let accessor = "package r;\n\npublic record R(int x) implements H {\n    int f() { return x(); }\n}\n";
    let none = "package r;\n\npublic record R(int y) implements H {\n    int f() { return x(); }\n}\n";
    flipped(("r/R.java", accessor), ("r/R.java", none), &[h], &call("sym:r/R.java::R.f", "sym:r/H.java::H.x"));
}

#[test]
fn a_created_type_added_removed_or_moved_is_followed_to_the_unchanged_file_that_creates_it() {
    let key = ("shop/Key.java", "package shop;\n\npublic class Key {}\n");
    let moved = ("shop/keys/Keys.java", "package shop;\n\npublic class Key {}\n");
    let other = ("shop/Other.java", "package shop;\n\nclass Other {}\n");
    let user = ("shop/Use.java", "package shop;\n\nclass Use {\n    Object go() { return new Key(); }\n}\n");
    let created = |edges: &Edges, to: &str| edges.contains(&("sym:shop/Use.java::Use.go".to_string(), to.to_string(), "Calls".to_string(), String::new()));
    let (updated, built) = updated_and_built(&[other, user], &[key]);
    assert!(created(&built, "sym:shop/Key.java::Key"), "{built:?}");
    assert_eq!(updated, built);
    let (updated, built) = removed_updated_and_built(&[key, other, user], &[], &[key.0]);
    assert!(!built.iter().any(|(s, _, k, _)| s == "sym:shop/Use.java::Use.go" && k == "Calls"), "{built:?}");
    assert_eq!(updated, built);
    let (updated, built) = removed_updated_and_built(&[key, other, user], &[moved], &[key.0]);
    assert!(created(&built, "sym:shop/keys/Keys.java::Key"), "{built:?}");
    assert_eq!(updated, built);
}

#[test]
fn a_member_type_a_superclass_gains_loses_or_hides_is_followed_to_the_unchanged_subclass() {
    let pkg = ("p/T.java", "package p;\n\npublic class T {}\n");
    let bare = ("p/B.java", "package p;\n\npublic class B {}\n");
    let nested = ("p/B.java", "package p;\n\npublic class B {\n    public static class T {}\n}\n");
    let hidden = ("p/B.java", "package p;\n\npublic class B {\n    private static class T {}\n}\n");
    let user = ("p/A.java", "package p;\n\nclass A extends B {\n    Object go() { return new T(); }\n}\n");
    let created = |edges: &Edges, to: &str| edges.contains(&("sym:p/A.java::A.go".to_string(), to.to_string(), "Calls".to_string(), String::new()));
    let (updated, built) = updated_and_built(&[pkg, bare, user], &[nested]);
    assert!(created(&built, "sym:p/B.java::B.T") && !created(&built, "sym:p/T.java::T"), "{built:?}");
    assert_eq!(updated, built);
    let (updated, built) = updated_and_built(&[pkg, nested, user], &[bare]);
    assert!(created(&built, "sym:p/T.java::T"), "{built:?}");
    assert_eq!(updated, built);
    let (updated, built) = updated_and_built(&[pkg, nested, user], &[hidden]);
    assert!(created(&built, "sym:p/T.java::T") && !created(&built, "sym:p/B.java::B.T"), "{built:?}");
    assert_eq!(updated, built);
}

#[test]
fn a_member_added_to_or_removed_from_a_receiver_s_supertype_in_another_file_is_followed_by_update() {
    let b = ("p/B.java", "package p;\n\npublic class B extends A {}\n");
    let u = ("p/U.java", "package p;\n\nclass U {\n    void go(B b) { b.run(1); }\n}\n");
    let plain = "package p;\n\npublic class A {}\n";
    let declaring = "package p;\n\npublic class A {\n    public void run(int x) {}\n}\n";
    flipped(("p/A.java", plain), ("p/A.java", declaring), &[b, u], &call("sym:p/U.java::U.go", "sym:p/A.java::A.run"));
    let b = ("app/B.kt", "package app\n\nopen class B : A()\n");
    let u = ("app/U.kt", "package app\n\nclass U {\n    fun go(b: B) { b.run(1) }\n}\n");
    let plain = "package app\n\nopen class A\n";
    let declaring = "package app\n\nopen class A {\n    fun run(x: Int) {}\n}\n";
    flipped(("app/A.kt", plain), ("app/A.kt", declaring), &[b, u], &call("sym:app/U.kt::U.go", "sym:app/A.kt::A.run"));
}

#[test]
fn a_receiver_type_made_a_data_class_or_given_another_record_component_is_followed_by_update() {
    let desc = ("app/Desc.kt", "package app\n\ninterface Desc {\n    fun copy(): Any = this\n}\n");
    let u = ("app/U.kt", "package app\n\nclass U {\n    fun go(d: D) { d.copy() }\n}\n");
    flipped(("app/D.kt", "package app\n\ndata class D(val a: Int) : Desc\n"), ("app/D.kt", "package app\n\nclass D(val a: Int) : Desc\n"), &[desc, u], &call("sym:app/U.kt::U.go", "sym:app/Desc.kt::Desc.copy"));
    let h = ("r/H.java", "package r;\n\npublic interface H {\n    default int x() { return -1; }\n}\n");
    let u = ("r/U.java", "package r;\n\nclass U {\n    int go(R r) { return r.x(); }\n}\n");
    flipped(("r/R.java", "package r;\n\npublic record R(int x) implements H {}\n"), ("r/R.java", "package r;\n\npublic record R(int y) implements H {}\n"), &[h, u], &call("sym:r/U.java::U.go", "sym:r/H.java::H.x"));
}
