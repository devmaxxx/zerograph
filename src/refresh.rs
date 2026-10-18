//! A reader's refresh held to a budget. `ask`, `impact`, `trace`, `changes` and `explain` bring
//! the store in line before they answer, and after a large pull that used to be minutes of
//! silence: 576 files re-read in 69.5 s, 1683 vectors embedded in 246.9 s. A reader now estimates
//! what it would do, answers from the store as it stands when that does not fit, and leaves the
//! work to one detached `update` that takes the store's writer lock.

use crate::walk;
use anyhow::Context as _;
use std::path::Path;
use std::time::{Duration, Instant};

/// What an answer was given without: the files the graph is behind the tree by, and the vector
/// rows owed to the graph it was answered from. `--json` carries it so an agent can read those
/// files itself instead of trusting an answer that cannot see them.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize)]
pub struct Stale { pub files: Vec<String>, pub vectors: usize }

/// A file re-read and settled. `apply_diff` cannot be stopped halfway, so the graph's side is an
/// estimate made before it starts: 576 files took 69.5 s on a machine at load average 47, and an
/// idle one is several times faster, so this lets about 250 files through a 10 s budget.
pub(crate) const FILE_COST: Duration = Duration::from_millis(40);

/// A row embedded by the default model on an idle machine: 144 s for the fixture's 33,525. Only
/// the first look uses it — the dense sync then measures its own rate chunk by chunk, which is
/// what a loaded machine needs, since the same rows ran at 147 ms each there.
pub(crate) const ROW_COST: Duration = Duration::from_millis(5);

/// When a reader has to have answered by, or no limit for a caller that is not a reader.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Budget { pub deadline: Option<Instant>, pub no_dense: bool }

impl Budget {
    pub(crate) fn seconds(s: u64, no_dense: bool) -> Budget {
        Budget { deadline: Instant::now().checked_add(Duration::from_secs(s)), no_dense }
    }

    #[cfg(test)]
    pub(crate) fn unlimited() -> Budget { Budget { deadline: None, no_dense: false } }

    /// Whether `units` of work at `cost` each would finish by the deadline. Nothing to do always
    /// fits, so a budget of 0 still answers a quiet tree without a line.
    pub(crate) fn fits(&self, units: usize, cost: Duration) -> bool {
        let Some(deadline) = self.deadline else { return true };
        let need = cost.checked_mul(u32::try_from(units).unwrap_or(u32::MAX)).unwrap_or(Duration::MAX);
        deadline.saturating_duration_since(Instant::now()) >= need
    }

    /// Whether the rest of a sync, at the rate its finished part ran, would end past the deadline.
    pub(crate) fn overrun(&self, done: usize, total: usize, spent: Duration) -> bool {
        let Some(deadline) = self.deadline else { return false };
        if done >= total { return false; }
        // A rate that overflows a Duration is a sync that will not finish, which is an overrun.
        let Ok(left) = Duration::try_from_secs_f64(spent.as_secs_f64() * ((total - done) as f64 / done.max(1) as f64)) else { return true };
        Instant::now().checked_add(left).is_none_or(|end| end > deadline)
    }
}

/// A sync stopped because the rest would not fit, with the rows it left owed.
#[derive(Debug)]
pub(crate) struct OutOfBudget(pub usize);

impl std::fmt::Display for OutOfBudget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{} rows left for a background refresh", self.0) }
}

impl std::error::Error for OutOfBudget {}

/// Every path the diff names, in the order a reader would read them.
pub(crate) fn files_of(diff: &walk::Diff) -> Vec<String> {
    let mut files: Vec<String> = diff.changed.iter().map(|e| e.rel.clone()).chain(diff.removed.iter().cloned()).collect();
    files.sort();
    files
}

/// The one line a reader prints when its answer is behind, after starting the refresh that will
/// catch it up — unless `busy`, when one is already running and a second would only queue behind
/// it.
pub(crate) fn left_behind(repo: &Path, no_dense: bool, what: &str, busy: bool) -> String {
    left_behind_as(repo, no_dense, what, busy, "refreshing")
}

/// `left_behind` with the running refresh named by `doing`, for a caller whose refresh is not a
/// catch-up but a move to another model.
pub(crate) fn left_behind_as(repo: &Path, no_dense: bool, what: &str, busy: bool, doing: &str) -> String {
    if busy { return format!("{what}, a refresh is already running"); }
    match spawn_update(repo, no_dense) {
        Ok(()) => format!("{what}, {doing} in background"),
        Err(err) => format!("{what}; the background refresh did not start ({err:#}) — run `zerograph update`"),
    }
}

pub(crate) fn files_line(n: usize) -> String {
    format!("index: {n} file{} behind", if n == 1 { "" } else { "s" })
}

pub(crate) fn vectors_line(n: usize) -> String {
    format!("dense: {n} vector{} pending, answered from the stored ones", if n == 1 { "" } else { "s" })
}

/// A JSON answer with `stale` added as its first field. Spliced into the text rather than
/// re-serialised: `serde_json::Value` would sort every key of an answer whose order is its
/// struct's, and a reader diffing two answers would see the whole object move. Anything that is
/// not an object is returned as it came.
pub(crate) fn with_stale(json: String, stale: &Stale) -> String {
    let Some(open) = json.find('{').filter(|&i| json[..i].trim().is_empty()) else { return json };
    let value = serde_json::to_string(stale).unwrap_or_default();
    let rest = &json[open + 1..];
    let empty = rest.trim_start().starts_with('}');
    let field = match rest.starts_with('\n') {
        // `to_string_pretty`'s layout, so a pretty answer stays one a person can read.
        true => format!("\n  \"stale\": {}{}", value, if empty { "\n" } else { "," }),
        false => format!("\"stale\":{}{}", value, if empty { "" } else { "," }),
    };
    format!("{}{field}{rest}", &json[..=open])
}

/// The log a detached refresh writes, kept from one run to the next until it passes this size,
/// then rotated to `background.log.1`: two refreshes can be queued on the lock at once, and
/// truncating at the second's start would cut the first one's lines off while it is still
/// writing them, where a renamed file keeps taking them under its new name.
const LOG_CAP: u64 = 1 << 20;

/// Starts `zerograph update` in a process of its own and returns without waiting.
pub(crate) fn spawn_update(repo: &Path, no_dense: bool) -> anyhow::Result<()> {
    spawn_detached(repo, no_dense, &["update"]).map(drop)
}

/// What a long-lived caller needs of a child it will not wait for: set while the process runs.
pub(crate) type Alive = std::sync::Arc<std::sync::atomic::AtomicBool>;

/// Waited for on a thread of its own: a server that spawns refreshes for days would otherwise
/// collect one zombie per refresh, and has no other moment to wait.
pub(crate) fn reap(mut child: std::process::Child) -> Alive {
    let alive = Alive::new(std::sync::atomic::AtomicBool::new(true));
    let flag = alive.clone();
    std::thread::spawn(move || {
        let _ = child.wait();
        flag.store(false, std::sync::atomic::Ordering::SeqCst);
    });
    alive
}

/// The line a refresh writes to its log once it holds the store, so a reader of the log can tell
/// this run's lines from those a killed run left.
pub(crate) const RUN_MARKER: &str = "refresh: started";

/// The line a run writes once it has done all it set out to. A run that lets go of the store
/// without it was killed or failed, whatever its last line says.
pub(crate) const RUN_FINISHED: &str = "refresh: finished";

/// Starts `zerograph <args>` in a process of its own, at background priority, and returns without
/// waiting. It inherits none of this process's standard streams: a caller reading this reader's
/// output through a pipe would otherwise wait for the refresh to close it too, which is the wait
/// this exists to remove.
pub(crate) fn spawn_detached(repo: &Path, no_dense: bool, args: &[&str]) -> anyhow::Result<Alive> {
    spawn_detached_logging(repo, no_dense, true, args, "background.log")
}

/// `spawn_detached` into a log of its own name: a detached `serve` runs for hours, and its lines
/// in `background.log` would sit between a refresh's marker and the progress read back after it.
/// `background` is false for that `serve`: a question can be waiting behind anything it does.
pub(crate) fn spawn_detached_logging(repo: &Path, no_dense: bool, background: bool, args: &[&str], log: &str) -> anyhow::Result<Alive> {
    let log = log_named(repo, log)?;
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    // No `current_dir`: `--repo` may be relative, and a child started inside it would resolve it
    // a second time.
    cmd.args(child_args(repo, no_dense, background, args))
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    Ok(reap(detach(&mut cmd)?))
}

/// `zerograph --background <args>` with no repository and no output anyone reads: housekeeping
/// that is the machine's, not a checkout's, so it has no store to log into. It starts in `dir`
/// rather than in this process's directory, which may be a worktree since removed.
pub(crate) fn spawn_quiet(dir: &Path, args: &[&str]) -> anyhow::Result<()> {
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    cmd.arg("--background").args(args).current_dir(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    reap(detach(&mut cmd)?);
    Ok(())
}

/// The child's command line: the global flags first, then the command.
fn child_args(repo: &Path, no_dense: bool, background: bool, args: &[&str]) -> Vec<std::ffi::OsString> {
    let mut out: Vec<std::ffi::OsString> = vec!["--repo".into(), repo.into()];
    if no_dense { out.push("--no-dense".into()); }
    if background { out.push("--background".into()); }
    out.extend(args.iter().map(|a| a.into()));
    out
}

#[cfg(test)]
fn log_file(repo: &Path) -> anyhow::Result<std::fs::File> { log_named(repo, "background.log") }

fn log_named(repo: &Path, name: &str) -> anyhow::Result<std::fs::File> {
    let p = crate::store::Store::new(repo).ensure_dir()?.join(name);
    let long = std::fs::metadata(&p).is_ok_and(|m| m.len() > LOG_CAP);
    // Windows refuses to rename a file another process holds open; that run keeps truncating.
    let rotated = long && std::fs::rename(&p, p.with_extension("log.1")).is_ok();
    let mut o = std::fs::OpenOptions::new();
    match long && !rotated {
        true => o.write(true).create(true).truncate(true),
        false => o.append(true).create(true),
    };
    crate::store::open_unlinked(&mut o, &p).with_context(|| format!("open {}", p.display()))
}

/// Its own process group, so the Ctrl-C a terminal sends the reader's group does not reach a
/// refresh that has nothing left to tell it.
#[cfg(unix)]
fn detach(cmd: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0).spawn()
}

/// No console and a process group of its own, so neither the console closing nor a Ctrl-C in it
/// ends the refresh. A job object the reader runs inside — a CI step, an agent harness — may kill
/// its members when it closes; breaking away from it is asked for first, and a job that refuses
/// breakaway fails the spawn, which is retried inside it rather than given up.
#[cfg(windows)]
fn detach(cmd: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    // CreateProcess passes every inheritable handle, and this process's own standard handles are
    // inheritable when its parent made them so: the child kept a caller's pipe open, and a caller
    // reading this command's output waited for the server to exit. std duplicates whatever a later
    // child is given, so clearing the flag on the originals costs no other spawn anything.
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT};
        for h in [std::io::stdin().as_raw_handle(), std::io::stdout().as_raw_handle(), std::io::stderr().as_raw_handle()] {
            if !h.is_null() { unsafe { SetHandleInformation(h as _, HANDLE_FLAG_INHERIT, 0) }; }
        }
    }
    match cmd.creation_flags(flags | CREATE_BREAKAWAY_FROM_JOB).spawn() {
        Ok(child) => Ok(child),
        Err(_) => cmd.creation_flags(flags).spawn(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stale() -> Stale { Stale { files: vec!["a.ts".into()], vectors: 3 } }

    #[test]
    fn stale_is_the_first_field_of_a_compact_object() {
        let out = with_stale("{\"touched\":[],\"risk\":\"LOW\"}\n".into(), &stale());
        assert_eq!(out, "{\"stale\":{\"files\":[\"a.ts\"],\"vectors\":3},\"touched\":[],\"risk\":\"LOW\"}\n");
        serde_json::from_str::<serde_json::Value>(&out).unwrap();
    }

    #[test]
    fn a_pretty_object_stays_pretty_and_valid() {
        let pretty = serde_json::to_string_pretty(&serde_json::json!({"seeds": [1]})).unwrap() + "\n";
        let out = with_stale(pretty, &stale());
        assert!(out.starts_with("{\n  \"stale\": {"), "{out}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["stale"]["vectors"], 3);
        assert_eq!(v["seeds"][0], 1);
    }

    #[test]
    fn an_empty_object_takes_the_field_without_a_trailing_comma() {
        for empty in ["{}", "{\n}"] {
            let v: serde_json::Value = serde_json::from_str(&with_stale(empty.into(), &stale())).unwrap();
            assert_eq!(v["stale"]["files"][0], "a.ts");
        }
    }

    #[test]
    fn a_log_over_the_cap_is_rotated_and_not_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join(".zerograph/background.log");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, vec![b'x'; LOG_CAP as usize + 1]).unwrap();
        drop(log_file(dir.path()).unwrap());
        assert_eq!(std::fs::metadata(log.with_extension("log.1")).unwrap().len(), LOG_CAP + 1, "the old lines are kept");
        assert_eq!(std::fs::metadata(&log).unwrap().len(), 0);
    }

    // The log is the one store file written by a process of its own, and a clone chooses what
    // sits at its name: a link there must not carry a refresh's output outside the repository.
    #[cfg(unix)]
    #[test]
    fn a_log_that_is_a_link_is_refused_and_its_target_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        std::fs::write(&outside, "victim\n").unwrap();
        let log = dir.path().join(".zerograph/background.log");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, &log).unwrap();
        assert!(log_file(dir.path()).is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "victim\n");
    }

    #[test]
    fn a_detached_refresh_runs_at_background_priority_and_a_detached_server_does_not() {
        let has = |background| child_args(Path::new("r"), false, background, &["update"]).iter().any(|a| a == "--background");
        assert!(has(true));
        assert!(!has(false));
        let parsed = <crate::Cli as clap::Parser>::try_parse_from(
            std::iter::once("zerograph".into()).chain(child_args(Path::new("r"), true, true, &["update"]))).unwrap();
        assert!(parsed.background && parsed.no_dense, "the child parses the flags it is given");
    }

    #[test]
    fn what_is_not_an_object_is_left_alone() {
        assert_eq!(with_stale("[1]".into(), &stale()), "[1]");
        assert_eq!(with_stale("no match\n".into(), &stale()), "no match\n");
    }

    #[test]
    fn nothing_to_do_fits_any_budget_and_anything_to_do_fits_none_at_zero() {
        let zero = Budget::seconds(0, false);
        assert!(zero.fits(0, FILE_COST));
        assert!(!zero.fits(1, FILE_COST));
        assert!(Budget::unlimited().fits(usize::MAX, FILE_COST));
        let ten = Budget::seconds(10, false);
        assert!(ten.fits(100, FILE_COST), "100 files is 4 s");
        assert!(!ten.fits(576, FILE_COST), "the pull the budget was agreed on is not");
    }

    #[test]
    fn a_sync_overruns_when_its_measured_rate_would_end_past_the_deadline() {
        let ten = Budget::seconds(10, false);
        assert!(!ten.overrun(100, 200, Duration::from_secs(1)), "one more second fits");
        assert!(ten.overrun(16, 1683, Duration::from_millis(2350)), "147 ms a row does not");
        assert!(!ten.overrun(5, 5, Duration::from_secs(60)), "a finished sync never overruns");
        assert!(!Budget::unlimited().overrun(1, 1_000_000, Duration::from_secs(60)));
        assert!(ten.overrun(1, usize::MAX, Duration::from_secs(u64::MAX / 2)), "a rate too large for a Duration is an overrun");
    }
}
