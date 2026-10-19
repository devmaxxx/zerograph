//! The binary over repositories the language layer reads nothing new in: the store it writes is
//! the store 0.5.3 wrote, and a glob that reaches past the grammars says so.

mod common;

use std::path::Path;

fn write(repo: &Path, rel: &str, text: &str) {
    let p = repo.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[test]
fn a_globbed_file_no_grammar_reads_is_named_once_on_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "zerograph.toml", "code_globs = [\"**/*.ts\", \"**/*.log\"]\n");
    write(repo, "web/a.ts", "export function boot() {}\n");
    write(repo, "svc/boot.log", "boot ok\n");
    write(repo, "svc/stop.log", "stop ok\n");
    let out = common::run(repo, &["build"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert_eq!(err.matches("code: no grammar reads 2 .log — indexed as files only").count(), 1, "{err}");
}

/// A family's readings take its language in before the defaults do, and write no `zerograph.toml`
/// into the tree they measure: the environment names the globs over the repository's own file.
#[test]
fn the_environment_names_the_globs_for_one_run_over_the_repository_s_own() {
    // `common::run` strips the variable, so these builds set their environment themselves.
    let build = |globs: Option<&str>| -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        write(repo, "zerograph.toml", "code_globs = [\"**/*.ts\"]\n");
        write(repo, "web/a.ts", "export function boot() {}\n");
        write(repo, "mobile/Pet.kt", "class Pet\n");
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_zerograph"));
        cmd.env_remove("ZEROGRAPH_BENCH_REPO").env_remove("ZEROGRAPH_EMBED_MODEL").env_remove("ZEROGRAPH_CODE_GLOBS");
        if let Some(g) = globs {
            cmd.env("ZEROGRAPH_CODE_GLOBS", g);
        }
        let out = cmd.arg("--no-dense").arg("--repo").arg(repo).arg("build").output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let graph: serde_json::Value = serde_json::from_slice(&std::fs::read(repo.join(".zerograph").join("graph.json")).unwrap()).unwrap();
        graph["nodes"].as_object().unwrap().keys().cloned().collect()
    };
    let own = build(None);
    let widened = build(Some("**/*.ts **/*.kt"));
    assert!(own.iter().any(|i| i == "file:web/a.ts") && !own.iter().any(|i| i == "file:mobile/Pet.kt"), "{own:?}");
    assert!(widened.iter().any(|i| i == "file:mobile/Pet.kt"), "{widened:?}");
}

#[test]
fn a_store_that_holds_no_name_indexed_file_carries_no_header_file() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "zerograph.toml", "code_globs = [\"**/*.ts\", \"**/*.cs\"]\n");
    write(repo, "web/a.ts", "export function boot() {}\n");
    write(repo, "svc/Program.cs", "public class Program {}\n");
    assert!(common::run(repo, &["build"]).status.success());
    write(repo, "web/a.ts", "export function boot() { return 1; }\n");
    std::fs::remove_file(repo.join("svc/Program.cs")).unwrap();
    assert!(common::run(repo, &["update"]).status.success());
    assert!(!repo.join(".zerograph").join("headers.json").exists());
}

#[test]
fn a_build_replaces_the_headers_an_older_store_left_at_its_save() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "web/a.ts", "export function boot() {}\n");
    assert!(common::run(repo, &["build"]).status.success());
    let path = repo.join(".zerograph").join("headers.json");
    std::fs::write(&path, r#"{"gone/A.java":{"package":["p"],"types":["A"]}}"#).unwrap();
    assert!(common::run(repo, &["build"]).status.success());
    assert!(!path.exists(), "a build reads beside no headers, so it leaves none the old graph was read beside");
}

/// The oversized count is said once, by the build, so a glob that reaches data files says how
/// many it left out instead of leaving the graph silently thinner than the tree.
#[test]
fn a_build_names_the_text_files_it_left_out_for_size() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "web/a.ts", "export function boot() {}\n");
    write(repo, "ops/deploy.yaml", "strategy: blue-green\n");
    write(repo, "data/dump.csv", &"a,b\n".repeat(300_000));
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zerograph"))
        .env_remove("ZEROGRAPH_BENCH_REPO").env_remove("ZEROGRAPH_EMBED_MODEL").env_remove("ZEROGRAPH_CODE_GLOBS")
        .env("ZEROGRAPH_TEXT_GLOBS", "**/*")
        .args(["--no-dense", "--repo"]).arg(repo).arg("build").output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert_eq!(err.matches("text: 1 file over 1 MiB left out").count(), 1, "{err}");
    let graph: serde_json::Value = serde_json::from_slice(&std::fs::read(repo.join(".zerograph/graph.json")).unwrap()).unwrap();
    assert_eq!(graph["nodes"]["file:ops/deploy.yaml"]["kind"], "Text");
    assert!(graph["nodes"].get("file:data/dump.csv").is_none());
}

fn git(repo: &Path, args: &[&str]) {
    let out = std::process::Command::new("git").arg("-C").arg(repo)
        .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE")
        .args(["-c", "user.name=zerograph tests", "-c", "user.email=tests@example.invalid", "-c", "commit.gpgsign=false"])
        .args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn with_text(repo: &Path, args: &[&str]) -> (String, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zerograph"))
        .env_remove("ZEROGRAPH_BENCH_REPO").env_remove("ZEROGRAPH_EMBED_MODEL").env_remove("ZEROGRAPH_CODE_GLOBS")
        .env("ZEROGRAPH_TEXT_GLOBS", "**/*")
        .args(["--no-dense", "--repo"]).arg(repo).args(args).output().unwrap();
    let (o, e) = (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned());
    assert!(out.status.success(), "{args:?}: {e}");
    (o, e)
}

/// T3: a text file is found by a phrase only it holds, and named when a diff touches it alone.
#[test]
fn a_text_file_is_found_by_what_it_says_and_named_when_it_changes() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "web/a.ts", "export function boot() {}\n");
    write(repo, "ops/deploy.yaml", "strategy: blue-green\nwindow: saturday-night\n");
    git(repo, &["init", "-q"]);
    git(repo, &["add", "."]);
    git(repo, &["commit", "-qm", "init"]);
    with_text(repo, &["build"]);
    let (asked, _) = with_text(repo, &["ask", "saturday night window"]);
    let top: Vec<&str> = asked.lines().filter(|l| !l.starts_with(' ')).take(5).collect();
    assert!(top.iter().any(|l| l.starts_with("ops/deploy.yaml:")), "{asked}");
    write(repo, "ops/deploy.yaml", "strategy: canary\nwindow: saturday-night\n");
    let (changed, _) = with_text(repo, &["changes", "--base", "HEAD"]);
    assert!(changed.contains("ops/deploy.yaml"), "{changed}");
}
