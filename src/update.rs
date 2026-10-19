//! A newer zerograph on npm, said once per interactive run and checked at most once a day.
//!
//! The answers live in `~/.cache/zerograph/update.json`, which this module owns: the session hook
//! and `status` read it without a request of their own, and its path is the same on every OS,
//! unlike the per-OS cache directory `update-informer` would otherwise keep its own file in. Each
//! release keeps its own entry, so two installs run in turn neither re-ask nor wipe each other.
//! A run reads the file before its command and prints from that; when its entry is a day old it
//! asks npm on a thread of its own that is never waited for, so no command takes longer for it.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use update_informer::http_client::{HttpClient, UreqHttpClient};
use update_informer::{registry, Check};

const DAY: u64 = 24 * 60 * 60;
/// How long a run that set out to ask holds off the others: one that exited before npm answered
/// is retried after this, not after a day.
const CLAIM: u64 = 60 * 60;
/// A request a terminal user never waits on, yet bounded so a hanging registry leaves no thread
/// behind for long in a run that outlives it, such as `ask` with a model to load.
const TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
pub(crate) struct Cache {
    /// Keyed by the release that asked.
    #[serde(default)]
    pub versions: BTreeMap<String, Entry>,
}

#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Eq, Clone)]
pub(crate) struct Entry {
    /// Seconds since the epoch when npm last answered.
    #[serde(default)]
    pub checked: Option<u64>,
    /// When a run last set out to ask.
    #[serde(default)]
    pub claimed: Option<u64>,
    /// What npm had that is newer than the entry's release, as of `checked`.
    #[serde(default)]
    pub latest: Option<String>,
}

/// SemVer's `MAJOR.MINOR.PATCH[-pre][+build]`, every part held to the identifier charset: the
/// version is repeated inside an install command a person may paste.
fn exact(v: &str) -> bool {
    let ident = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    let (rest, build) = match v.split_once('+') { Some((r, b)) => (r, Some(b)), None => (v, None) };
    let (core, pre) = match rest.split_once('-') { Some((c, p)) => (c, Some(p)), None => (rest, None) };
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && pre.is_none_or(ident) && build.is_none_or(ident)
}

pub(crate) fn path_under(home: &Path) -> PathBuf { home.join(".cache").join("zerograph").join("update.json") }

fn path() -> Option<PathBuf> { std::env::home_dir().filter(|h| h.is_absolute()).map(|h| path_under(&h)) }

/// The file as written, less any entry that names a version that is not one; a missing or
/// garbled file reads as empty.
pub(crate) fn read_from(path: &Path) -> Cache {
    let mut c: Cache = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    c.versions.retain(|v, e| exact(v) && e.latest.as_deref().is_none_or(exact));
    c
}

/// Through a file beside it and a rename, so a reader never finds it half written.
fn write_to(path: &Path, c: &Cache) {
    let Some(dir) = path.parent() else { return };
    let _ = std::fs::create_dir_all(dir);
    let tmp = dir.join(format!(".update.json.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, serde_json::to_vec(c).unwrap_or_default()).is_ok() && std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// The newer release npm last named for a binary at `own`.
pub(crate) fn newer<'a>(c: &'a Cache, own: &str) -> Option<&'a str> {
    let e = c.versions.get(own)?;
    e.checked?;
    e.latest.as_deref()
}

/// Whether a run at `own` should ask npm at `now`. A time ahead of `now` counts as old: a clock
/// set back would otherwise hold the answer until it caught up.
pub(crate) fn due(c: &Cache, own: &str, now: u64) -> bool {
    let older = |t: Option<u64>, by: u64| t.is_none_or(|t| now < t || now - t >= by);
    c.versions.get(own).is_none_or(|e| older(e.checked, DAY) && older(e.claimed, CLAIM))
}

/// The variables that turn the check off for a person.
pub(crate) fn opted_out(var: impl Fn(&str) -> bool) -> bool {
    var("ZEROGRAPH_NO_UPDATE_CHECK") || var("NO_UPDATE_NOTIFIER")
}

fn set(name: &str) -> bool { std::env::var_os(name).is_some() }

/// Whether this run may ask npm and print. `quiet` is a command whose streams belong to another
/// program (`mcp`, `serve`) or that reports the fact itself (`status`); CI is a machine nobody
/// reads the box on.
pub(crate) fn wanted(quiet: bool, json: bool, var: impl Fn(&str) -> bool, stderr_tty: bool) -> bool {
    !quiet && !json && stderr_tty && !var("CI") && !opted_out(var)
}

/// For `status`: the cached fact, read and never fetched.
pub(crate) fn known() -> Option<String> {
    if opted_out(set) { return None; }
    newer(&read_from(&path()?), env!("CARGO_PKG_VERSION")).map(str::to_string)
}

pub(crate) fn commands(latest: &str) -> (String, String) {
    (format!("npm i -g zerograph@{latest}"), format!("pnpm add -g zerograph@{latest}"))
}

/// npm update-notifier's box: rounded, every line centred, as wide as its longest line. Every
/// part is ASCII but the arrow, one column, so a char count is the display width.
pub(crate) fn render(current: &str, latest: &str, color: bool) -> String {
    let paint = |code: &str, s: &str| if color { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() };
    let (npm, pnpm) = commands(latest);
    let lines: [(String, String); 3] = [
        (format!("Update available {current} → {latest}"), format!("Update available {} → {}", paint("2", current), paint("32", latest))),
        (format!("Run {npm}"), format!("Run {}", paint("36", &npm))),
        (format!("or {pnpm}"), format!("or {}", paint("36", &pnpm))),
    ];
    let inner = lines.iter().map(|(plain, _)| plain.chars().count()).max().unwrap_or(0) + 6;
    let side = paint("33", "│");
    let blank = format!("{side}{}{side}", " ".repeat(inner));
    let mut out = vec![paint("33", &format!("╭{}╮", "─".repeat(inner))), blank.clone()];
    for (plain, shown) in &lines {
        let gap = inner - plain.chars().count();
        out.push(format!("{side}{}{shown}{}{side}", " ".repeat(gap / 2), " ".repeat(gap - gap / 2)));
    }
    out.push(blank);
    out.push(paint("33", &format!("╰{}╯", "─".repeat(inner))));
    out.join("\n")
}

/// Colour as update-notifier would, unless the person or the terminal declines it.
fn color(var: impl Fn(&str) -> Option<String>) -> bool {
    var("NO_COLOR").is_none() && var("CLICOLOR").as_deref() != Some("0") && var("TERM").as_deref() != Some("dumb")
}

/// npm's answer for `own`, through `H`: the newer release, or `None` when there is none.
fn fetch<H: HttpClient>(client: H, own: &str) -> update_informer::Result<Option<String>> {
    // `Duration::ZERO` keeps the crate's own cache file out of it: this module keeps one.
    let found = update_informer::new(registry::Npm, "zerograph", own).interval(Duration::ZERO).timeout(TIMEOUT).http_client(client).check_version()?;
    Ok(found.map(|v| v.semver().to_string()))
}

/// Claims the hour, asks npm, and records the answer once it comes. The file is read again before
/// the answer is written, so what another release's run wrote meanwhile is kept.
pub(crate) fn refresh_with<H: HttpClient>(client: H, path: &Path, own: &str, now: u64) {
    let mut c = read_from(path);
    c.versions.entry(own.to_string()).or_default().claimed = Some(now);
    write_to(path, &c);
    let Ok(latest) = fetch(client, own) else { return };
    let mut c = read_from(path);
    c.versions.insert(own.to_string(), Entry { checked: Some(now), claimed: None, latest: latest.filter(|v| exact(v)) });
    write_to(path, &c);
}

/// What a run at `own` does about the cache at `path`: the box to print, if any, and the refresh
/// it started, if one was due.
pub(crate) fn start_with<H: HttpClient + Send + 'static>(client: H, path: &Path, own: &'static str, now: u64, color: bool) -> (Option<String>, Option<std::thread::JoinHandle<()>>) {
    let c = read_from(path);
    let started = due(&c, own, now).then(|| {
        let p = path.to_path_buf();
        std::thread::spawn(move || refresh_with(client, &p, own, now))
    });
    let shown = newer(&c, own).map(|latest| render(&crate::terminal_safe(own), &crate::terminal_safe(latest), color));
    (shown, started)
}

static NOTICE: OnceLock<String> = OnceLock::new();

/// Called once a run knows its command: starts a check that is due and keeps the box for `print`.
pub(crate) fn start(quiet: bool, json: bool) {
    use std::io::IsTerminal;
    if !wanted(quiet, json, set, std::io::stderr().is_terminal()) { return; }
    let Some(path) = path() else { return };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (shown, _never_joined) = start_with(UreqHttpClient, &path, env!("CARGO_PKG_VERSION"), now, color(|k| std::env::var(k).ok()));
    if let Some(s) = shown { let _ = NOTICE.set(s); }
}

/// The box `start` kept, on stderr, after the command's own output.
pub(crate) fn print() {
    if let Some(n) = NOTICE.get() { eprintln!("{n}"); }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: &str = "0.7.0";

    fn vars(set: &'static [&'static str]) -> impl Fn(&str) -> bool { move |k| set.contains(&k) }

    #[test]
    fn the_check_is_skipped_for_machine_streams_json_ci_opt_outs_and_no_terminal() {
        assert!(wanted(false, false, vars(&[]), true));
        assert!(!wanted(true, false, vars(&[]), true), "mcp, serve, status");
        assert!(!wanted(false, true, vars(&[]), true), "--json");
        assert!(!wanted(false, false, vars(&[]), false), "stderr not a terminal");
        assert!(!wanted(false, false, vars(&["CI"]), true));
        assert!(!wanted(false, false, vars(&["ZEROGRAPH_NO_UPDATE_CHECK"]), true));
        assert!(!wanted(false, false, vars(&["NO_UPDATE_NOTIFIER"]), true));
    }

    fn entry(checked: Option<u64>, claimed: Option<u64>, latest: Option<&str>) -> Entry {
        Entry { checked, claimed, latest: latest.map(str::to_string) }
    }

    fn cache(pairs: &[(&str, Entry)]) -> Cache { Cache { versions: pairs.iter().map(|(v, e)| (v.to_string(), e.clone())).collect() } }

    #[test]
    fn a_missing_or_garbled_cache_reads_empty_and_a_bad_version_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let p = path_under(dir.path());
        assert_eq!(read_from(&p), Cache::default());
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        for body in ["", "{", "null", r#"{"versions":{"0.7.0":{"checked":1,"latest":"0.7.1; rm -rf ~"}}}"#, r#"{"versions":{"x":{"checked":1}}}"#] {
            std::fs::write(&p, body).unwrap();
            assert_eq!(read_from(&p), Cache::default(), "{body}");
        }
        std::fs::write(&p, r#"{"versions":{"0.7.0":{"checked":1,"latest":"0.7.1"}}}"#).unwrap();
        assert_eq!(newer(&read_from(&p), OWN), Some("0.7.1"));
    }

    #[test]
    fn an_answer_is_about_the_release_that_asked() {
        let c = cache(&[(OWN, entry(Some(1), None, Some("0.7.1")))]);
        assert_eq!(newer(&c, OWN), Some("0.7.1"));
        assert_eq!(newer(&c, "0.7.1"), None, "after the upgrade");
        assert_eq!(newer(&c, "0.6.0"), None, "after a downgrade");
    }

    #[test]
    fn a_check_is_due_daily_and_a_claim_holds_the_others_off_for_an_hour() {
        let t = 1_000_000;
        assert!(due(&Cache::default(), OWN, t));
        let answered = cache(&[(OWN, entry(Some(t), None, None))]);
        assert!(!due(&answered, OWN, t + DAY - 1));
        assert!(due(&answered, OWN, t + DAY));
        assert!(due(&answered, OWN, t - 1), "a clock set back");
        assert!(due(&answered, "0.7.1", t + 1), "another release has its own entry");
        let claimed = cache(&[(OWN, entry(Some(t), Some(t + DAY), None))]);
        assert!(!due(&claimed, OWN, t + DAY + CLAIM - 1));
        assert!(due(&claimed, OWN, t + DAY + CLAIM), "a run that exited before npm answered is retried");
    }

    #[test]
    fn the_box_matches_update_notifiers_in_plain_text() {
        let expected = "\
╭────────────────────────────────────╮
│                                    │
│   Update available 0.7.0 → 0.7.1   │
│    Run npm i -g zerograph@0.7.1    │
│   or pnpm add -g zerograph@0.7.1   │
│                                    │
╰────────────────────────────────────╯";
        assert_eq!(render(OWN, "0.7.1", false), expected);
    }

    #[test]
    fn a_longer_version_widens_the_box_and_every_row_stays_centred() {
        for shown in [render("0.7.0", "10.20.300-rc.1+build.7", false), render("0.7.0", "0.7.1", false)] {
            let widths: Vec<usize> = shown.lines().map(|l| l.chars().count()).collect();
            assert!(widths.windows(2).all(|w| w[0] == w[1]), "{shown}");
            let rows: Vec<&str> = shown.lines().collect();
            let longest = rows[2..rows.len() - 2].iter().map(|l| l.trim_matches('│').trim().chars().count()).max().unwrap();
            assert_eq!(widths[0], longest + 8, "{shown}");
            for line in &rows[2..rows.len() - 2] {
                let inner: Vec<char> = line.chars().skip(1).take(line.chars().count() - 2).collect();
                let left = inner.iter().take_while(|c| **c == ' ').count();
                let right = inner.iter().rev().take_while(|c| **c == ' ').count();
                assert!(left == right || left + 1 == right, "{line}");
            }
        }
    }

    #[test]
    fn colour_only_where_the_terminal_takes_it() {
        assert!(render(OWN, "0.7.1", true).contains("\x1b[32m0.7.1\x1b[0m"));
        assert!(!render(OWN, "0.7.1", false).contains('\x1b'));
        let env = |pairs: &'static [(&'static str, &'static str)]| move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
        assert!(color(env(&[("TERM", "xterm-256color")])));
        assert!(!color(env(&[("NO_COLOR", "")])));
        assert!(!color(env(&[("CLICOLOR", "0")])));
        assert!(!color(env(&[("TERM", "dumb")])));
    }

    macro_rules! registry {
        ($name:ident, $body:expr) => {
            struct $name;
            impl HttpClient for $name {
                fn get<T: serde::de::DeserializeOwned>(url: &str, _: Duration, _: update_informer::http_client::HeaderMap) -> update_informer::Result<T> {
                    assert_eq!(url, "https://registry.npmjs.org/zerograph/latest");
                    let body: Option<&str> = $body;
                    Ok(serde_json::from_str(body.ok_or("offline")?)?)
                }
            }
        };
    }
    registry!(Newer, Some(r#"{"version":"0.7.1"}"#));
    registry!(Same, Some(r#"{"version":"0.7.0"}"#));
    registry!(Offline, None);

    #[test]
    fn an_answer_is_recorded_only_once_npm_gives_it() {
        let dir = tempfile::tempdir().unwrap();
        let p = path_under(dir.path());
        refresh_with(Newer, &p, OWN, 100);
        assert_eq!(read_from(&p).versions[OWN], entry(Some(100), None, Some("0.7.1")));
        refresh_with(Offline, &p, OWN, 100 + DAY);
        assert_eq!(read_from(&p).versions[OWN], entry(Some(100), Some(100 + DAY), Some("0.7.1")), "claimed, the old answer not re-stamped");
        refresh_with(Same, &p, OWN, 100 + DAY + CLAIM);
        assert_eq!(read_from(&p).versions[OWN], entry(Some(100 + DAY + CLAIM), None, None));
    }

    #[test]
    fn two_releases_keep_their_own_entries() {
        let dir = tempfile::tempdir().unwrap();
        let p = path_under(dir.path());
        refresh_with(Newer, &p, OWN, 100);
        refresh_with(Same, &p, "0.7.1", 200);
        let c = read_from(&p);
        assert_eq!(newer(&c, OWN), Some("0.7.1"));
        assert!(!due(&c, OWN, 300) && !due(&c, "0.7.1", 300));
        assert!(!std::fs::read_dir(p.parent().unwrap()).unwrap().any(|f| f.unwrap().file_name().to_string_lossy().ends_with(".tmp")), "no temporary file is left");
    }

    #[test]
    fn a_run_prints_what_is_cached_and_refreshes_only_when_due() {
        let dir = tempfile::tempdir().unwrap();
        let p = path_under(dir.path());
        let (shown, started) = start_with(Newer, &p, OWN, 100, false);
        assert_eq!(shown, None, "nothing known yet");
        started.expect("a first run asks").join().unwrap();
        let (shown, started) = start_with(Offline, &p, OWN, 200, false);
        assert_eq!(shown.as_deref(), Some(render(OWN, "0.7.1", false).as_str()));
        assert!(started.is_none(), "answered today");
        let (shown, _) = start_with(Offline, &p, "0.7.1", 200, false);
        assert_eq!(shown, None, "the upgraded binary has no newer release to name");
    }
}
