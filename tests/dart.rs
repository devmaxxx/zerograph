//! The binary over a Dart repository: two packages, a barrel, a typed field and an import that is
//! never used.

use std::path::Path;
use std::process::Command;

/// The binary as `tests/common` spawns it, with the two bench variables removed for the same reason,
/// and Dart globbed through the environment so the repository needs no `zerograph.toml`.
fn zerograph(repo: &Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_zerograph"))
        .env_remove("ZEROGRAPH_BENCH_REPO")
        .env_remove("ZEROGRAPH_EMBED_MODEL")
        .env("ZEROGRAPH_CODE_GLOBS", "**/*.dart")
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

fn shop(repo: &Path) {
    write(repo, "packages/core/pubspec.yaml", "name: core\n");
    write(repo, "packages/core/lib/orders.dart", "export 'src/orders_impl.dart' show Orders;\n");
    write(repo, "packages/core/lib/src/orders_impl.dart", "class Orders {\n  void place() {}\n}\n");
    write(repo, "app/pubspec.yaml", "name: shop\n");
    write(repo, "app/lib/main.dart", "import 'package:core/orders.dart';\nimport 'state/cart.dart';\n\nclass Home {\n  late final Orders _orders;\n  final _cart = Cart();\n\n  void checkout() {\n    _cart.add(1);\n    _orders.place();\n  }\n}\n");
    write(repo, "app/lib/state/cart.dart", "class Cart {\n  void add(int n) {}\n}\n");
    write(repo, "app/lib/idle.dart", "import 'package:core/orders.dart';\n\nclass Idle {}\n");
    let (ok, out, err) = zerograph(repo, &["build"]);
    assert!(ok, "{out}{err}");
}

#[test]
fn impact_names_the_importer_and_the_barrel_and_not_a_file_that_imports_without_using() {
    let dir = tempfile::tempdir().unwrap();
    shop(dir.path());
    let (ok, out, err) = zerograph(dir.path(), &["impact", "Orders"]);
    assert!(ok, "{err}");
    assert!(out.contains("app/lib/main.dart"), "main.dart holds an Orders field and calls it: {out}");
    assert!(out.contains("packages/core/lib/orders.dart"), "the barrel re-exports it: {out}");
    assert!(!out.contains("app/lib/idle.dart"), "an import that uses nothing is not a dependent: {out}");
}

#[test]
fn trace_crosses_packages_through_a_typed_field() {
    let dir = tempfile::tempdir().unwrap();
    shop(dir.path());
    let (ok, out, err) = zerograph(dir.path(), &["trace", "Home", "Orders"]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("Orders.place"), "{out}");
}

#[test]
fn a_body_only_edit_re_reads_that_file_alone() {
    let dir = tempfile::tempdir().unwrap();
    shop(dir.path());
    write(dir.path(), "app/lib/state/cart.dart", "class Cart {\n  void add(int n) {\n    n;\n  }\n}\n");
    let (ok, out, err) = zerograph(dir.path(), &["update"]);
    assert!(ok, "{err}");
    assert!(out.starts_with("changed 1 removed 0"), "{out}");
}
