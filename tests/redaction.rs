//! Credentials inside indexed files, end to end: what a build stores, what `families` prints, and
//! the switch a project turns redaction off with.

mod common;

use common::{run, write};

// Assembled at run time so this file holds no string a scanner of this repository flags.
fn token() -> String { format!("ghp_{}", "R7xK2mQ9vT4wL8nB3cF6hJ1pS5dG0aZyEuXo") }

fn ops(token: &str) -> String { format!("# Ops\n\n**OPS-1 · MUST · Rotate {token} monthly**\n\nDeploy key for NEW-5: {token}\n") }

fn out(repo: &std::path::Path, args: &[&str], env: &[(&str, &str)]) -> String {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_zerograph"));
    cmd.env_remove("ZEROGRAPH_REDACT_SECRETS").env_remove("ZEROGRAPH_EMBED_MODEL").env_remove("ZEROGRAPH_BENCH_REPO")
        .env_remove("ZEROGRAPH_CODE_GLOBS").env_remove("ZEROGRAPH_TEXT_GLOBS").env_remove("ZEROGRAPH_READER_BUDGET");
    for (k, v) in env { cmd.env(k, v); }
    let o = cmd.arg("--no-dense").arg("--repo").arg(repo).args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(o.status.success(), "{args:?}: {stdout}{}", String::from_utf8_lossy(&o.stderr));
    stdout
}

#[test]
fn a_comment_that_says_key_beside_a_requirement_keeps_its_reference() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "docs/cal.md", "# Calendar\n\n**FR-CAL-155 · MUST · Past slots are refused**\n");
    write(repo, "src/rules.ts", "export function rules() {\n  /**\n   * Against `when.now`, which is the whole reason that key exists: `FR-CAL-155` makes this a pure\n   */\n  return 1;\n}\n");
    write(repo, "src/rbac.ts", "export async function scoped(tx: Tx) {\n  return tx.query<{ key: PermissionKey }>(`SELECT key FROM permissions`);\n}\n");
    let built = run(repo, &["build"]);
    let err = String::from_utf8_lossy(&built.stderr);
    assert!(built.status.success(), "{err}");
    assert!(!err.contains("redacted"), "nothing in these files is a secret: {err}");
    let explained = out(repo, &["explain", "FR-CAL-155"], &[]);
    assert!(explained.contains("References ← sym:src/rules.ts::rules"), "{explained}");
}

#[test]
fn a_provider_token_is_redacted_unless_the_project_or_the_run_turns_it_off() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "docs/ops.md", &ops(&token()));

    out(repo, &["build"], &[]);
    let explained = out(repo, &["explain", "OPS-1"], &[]);
    assert!(explained.contains("Rotate [REDACTED_SECRET] monthly") && !explained.contains(&token()), "{explained}");

    write(repo, "zerograph.toml", "redact_secrets = false\n");
    out(repo, &["build"], &[]);
    let explained = out(repo, &["explain", "OPS-1"], &[]);
    assert!(explained.contains(&format!("Rotate {} monthly", token())), "the project's switch: {explained}");

    out(repo, &["build"], &[("ZEROGRAPH_REDACT_SECRETS", "true")]);
    let explained = out(repo, &["explain", "OPS-1"], &[]);
    assert!(!explained.contains(&token()), "the environment outranks the file for one run: {explained}");

    let refused = std::process::Command::new(env!("CARGO_BIN_EXE_zerograph"))
        .env("ZEROGRAPH_REDACT_SECRETS", "sometimes").arg("--no-dense").arg("--repo").arg(repo).arg("build").output().unwrap();
    assert!(!refused.status.success() && String::from_utf8_lossy(&refused.stderr).contains("ZEROGRAPH_REDACT_SECRETS"), "a value that is neither is refused by name");
}

#[test]
fn families_prints_its_example_line_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "docs/ops.md", &ops(&token()));
    out(repo, &["build"], &[]);

    let report = out(repo, &["families"], &[]);
    assert!(report.contains("Deploy key for NEW-5: [REDACTED_SECRET]"), "{report}");
    assert!(!report.contains("ghp_"), "{report}");

    let report = out(repo, &["families"], &[("ZEROGRAPH_REDACT_SECRETS", "0")]);
    assert!(report.contains("Deploy key for NEW-5: ghp_"), "and the switch reaches it too: {report}");
}
