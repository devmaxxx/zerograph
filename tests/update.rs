//! The daily check for a newer release, seen from outside: `status` reports what the cache holds
//! without asking npm, a run with no terminal on stderr neither asks nor prints, and one with a
//! terminal ends with the box.

use std::path::Path;
use std::process::{Command, Output};

const OWN: &str = env!("CARGO_PKG_VERSION");
const NEWER: &str = "999.0.0";

fn cache(home: &Path) -> std::path::PathBuf { home.join(".cache/zerograph/update.json") }

fn plant(home: &Path, body: &str) {
    std::fs::create_dir_all(cache(home).parent().unwrap()).unwrap();
    std::fs::write(cache(home), body).unwrap();
}

fn fresh(current: &str) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    format!(r#"{{"versions":{{"{current}":{{"checked":{now},"latest":"{NEWER}"}}}}}}"#)
}

fn run(home: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let repo = home.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_zerograph"));
    cmd.current_dir(&repo).env("HOME", home).env("USERPROFILE", home)
        .env_remove("ZEROGRAPH_NO_UPDATE_CHECK").env_remove("NO_UPDATE_NOTIFIER").env_remove("CI");
    for (k, v) in env { cmd.env(k, v); }
    cmd.args(args).output().unwrap()
}

fn text(b: &[u8]) -> String { String::from_utf8_lossy(b).into_owned() }

#[test]
fn status_reports_the_cached_release_in_text_and_json() {
    let home = tempfile::tempdir().unwrap();
    plant(home.path(), &fresh(OWN));
    let out = run(home.path(), &["status"], &[]);
    let said = text(&out.stdout);
    assert!(said.contains(&format!("update: zerograph {NEWER} is out and this is {OWN}")), "{said}");
    assert!(said.contains(&format!("npm i -g zerograph@{NEWER}")) && said.contains(&format!("pnpm add -g zerograph@{NEWER}")), "{said}");
    let json: serde_json::Value = serde_json::from_slice(&run(home.path(), &["status", "--json"], &[]).stdout).unwrap();
    assert_eq!(json["update"], NEWER);
}

#[test]
fn status_is_silent_when_the_cache_is_missing_garbled_stale_for_this_release_or_opted_out() {
    for (body, env) in [
        (None, vec![]),
        (Some("{not json".to_string()), vec![]),
        (Some(fresh("0.0.1")), vec![]),
        (Some(fresh(OWN)), vec![("ZEROGRAPH_NO_UPDATE_CHECK", "1")]),
        (Some(fresh(OWN)), vec![("NO_UPDATE_NOTIFIER", "1")]),
    ] {
        let home = tempfile::tempdir().unwrap();
        if let Some(b) = &body { plant(home.path(), b); }
        let out = run(home.path(), &["status", "--json"], &env);
        assert!(out.status.success(), "{}", text(&out.stderr));
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(json["update"].is_null(), "{body:?} {env:?}: {json}");
    }
}

#[test]
fn a_run_without_a_terminal_neither_asks_nor_prints() {
    let home = tempfile::tempdir().unwrap();
    let out = run(home.path(), &["families"], &[]);
    assert!(!cache(home.path()).exists(), "no check was started: {}", text(&out.stderr));
    plant(home.path(), &fresh(OWN));
    let out = run(home.path(), &["families"], &[]);
    for stream in [text(&out.stdout), text(&out.stderr)] {
        assert!(!stream.contains("Update available"), "{stream}");
    }
}

/// `zerograph <args>` under a pseudo-terminal, through `script`, whose arguments differ between the
/// BSD and util-linux builds. The cache is fresh, so no request is made.
#[cfg(unix)]
#[test]
fn a_run_on_a_terminal_ends_with_the_box_on_stderr() {
    let home = tempfile::tempdir().unwrap();
    plant(home.path(), &fresh(OWN));
    let bin = env!("CARGO_BIN_EXE_zerograph");
    let mut cmd = Command::new("script");
    if cfg!(target_os = "linux") { cmd.args(["-qec", &format!("'{bin}' families"), "/dev/null"]); } else { cmd.args(["-q", "/dev/null", bin, "families"]); }
    let repo = home.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    cmd.current_dir(&repo).env("HOME", home.path()).env("NO_COLOR", "1")
        .env_remove("ZEROGRAPH_NO_UPDATE_CHECK").env_remove("NO_UPDATE_NOTIFIER").env_remove("CI")
        .stdin(std::process::Stdio::null());
    let Ok(out) = cmd.output() else { return };
    let said = text(&out.stdout);
    assert!(said.contains(&format!("Update available {OWN} → {NEWER}")), "{said}");
    assert!(said.contains(&format!("or pnpm add -g zerograph@{NEWER}")), "{said}");
    let after_error = said.find("graph is empty").is_none_or(|e| said.find("Update available").unwrap() > e);
    assert!(after_error, "the box is the last thing said: {said}");
}
