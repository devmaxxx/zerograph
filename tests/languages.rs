//! A repository with no `zerograph.toml` reads every language 0.6.0 ships, each through its own
//! extractor, keeps a minified bundle out through the default `skip`, and leaves Razor to a glob
//! naming it, since Razor failed its readings.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

const TREE: &[(&str, &str)] = &[
    ("web/app.ts", "export function boot() {}\n"),
    ("web/vendor.min.js", "export function packed() {}\n"),
    ("web/hooks.mjs", "export function hook() {}\n"),
    ("mobile/Pet.kt", "package app\n\nclass Pet\n"),
    ("android/Shell.java", "package app;\n\npublic class Shell {}\n"),
    ("svc/Order.cs", "namespace Shop.Orders;\n\npublic class Order {}\n"),
    ("svc/Card.razor", "<h1>Card</h1>\n@code {\n    private int count;\n}\n"),
    ("crates/core/Cargo.toml", "[package]\nname = \"core\"\nversion = \"0.1.0\"\n"),
    ("crates/core/src/lib.rs", "pub fn run() {}\n"),
    ("tools/report.py", "def report():\n    return 1\n"),
    ("app/lib/pet.dart", "class Pet {}\n"),
    ("app/ios/Runner.swift", "struct Runner {}\n"),
    ("graphql/cards.gql", "query Cards {\n  cards {\n    id\n  }\n}\n"),
    ("db/0001_clients.sql", "CREATE TABLE app.clients (id int, status text);\n"),
    ("azure/main.bicep", "param location string\n"),
    ("infra/main.tf", "resource \"aws_instance\" \"web\" {\n  ami = \"x\"\n}\n"),
    ("docker-bake.hcl", "target \"api\" {\n  context = \".\"\n}\n"),
    ("scripts/deploy.sh", "settle() {\n  :\n}\n"),
    ("tools/.venv/lib/site.py", "def vendored():\n    return 1\n"),
    ("crates/core/target/debug/build/out.rs", "pub fn generated() {}\n"),
    ("web/Card.vue", "<template><p/></template>\n<script lang=\"ts\">\nexport default {}\n</script>\n"),
];

const EXPECTED: &[&str] = &[
    "sym:web/app.ts::boot",
    "sym:web/hooks.mjs::hook",
    "sym:mobile/Pet.kt::Pet",
    "sym:android/Shell.java::Shell",
    "sym:svc/Order.cs::Order",
    "sym:crates/core/src/lib.rs::run",
    "sym:tools/report.py::report",
    "sym:app/lib/pet.dart::Pet",
    "sym:app/ios/Runner.swift::Runner",
    "sym:graphql/cards.gql::query/Cards",
    "sym:db/0001_clients.sql::app/clients",
    "sym:azure/main.bicep::location",
    "sym:infra/main.tf::aws_instance/web",
    "sym:docker-bake.hcl::target/api",
    "sym:scripts/deploy.sh::settle",
    "sym:web/Card.vue::Card",
];

fn write_tree(repo: &Path) {
    for (rel, text) in TREE {
        let p = repo.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
}

fn build(repo: &Path) -> BTreeSet<String> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_zerograph"));
    // As `tests/common` strips them: globs a shell exported would stand in for the defaults under test.
    cmd.env_remove("ZEROGRAPH_BENCH_REPO").env_remove("ZEROGRAPH_EMBED_MODEL")
        .env_remove("ZEROGRAPH_CODE_GLOBS").env_remove("ZEROGRAPH_TEXT_GLOBS");
    let out = cmd.arg("--no-dense").arg("--repo").arg(repo).arg("build").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let graph: serde_json::Value = serde_json::from_slice(&std::fs::read(repo.join(".zerograph").join("graph.json")).unwrap()).unwrap();
    graph["nodes"].as_object().unwrap().keys().cloned().collect()
}

#[test]
fn a_repository_with_no_config_reads_every_default_language() {
    let dir = tempfile::tempdir().unwrap();
    write_tree(dir.path());
    let ids = build(dir.path());
    for id in EXPECTED {
        assert!(ids.contains(*id), "{id} missing from {ids:?}");
    }
    assert!(!ids.contains("file:web/vendor.min.js"), "a bundle is skipped by default: {ids:?}");
    assert!(!ids.iter().any(|id| id.contains("/.venv/") || id.contains("/target/")), "a virtualenv and a build directory are skipped by default: {ids:?}");
    assert!(!ids.iter().any(|id| id.contains("svc/Card.razor")), "Razor is read only where a glob names it: {ids:?}");
}
