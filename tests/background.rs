//! What runs at background priority and what does not, read from outside the process the way
//! another program on the machine sees it: a detached refresh is lowered.
#![cfg(any(target_os = "macos", target_os = "linux"))]

mod common;

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn built() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    common::write(dir.path(), "docs/a.md", "# A\n\n**FR-PAY-1 · MUST · cancellation window**\n\nbody\n");
    assert!(common::run(dir.path(), &["build"]).status.success());
    dir
}

fn start(repo: &std::path::Path, args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_zerograph"))
        .arg("--no-dense").arg("--repo").arg(repo).args(args)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().unwrap()
}

/// Whether another process is in the background band. `getpriority(PRIO_DARWIN_PROCESS)` reads
/// back 0 for a process that lowered itself when another one asks, so this reads what `ps` sees:
/// the throttled band's scheduling priority, 4, against 31 for a normal one.
#[cfg(target_os = "macos")]
fn lowered(pid: u32) -> bool {
    let out = Command::new("ps").args(["-o", "pri=", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse::<u32>().is_ok_and(|pri| pri <= 4)
}

#[cfg(target_os = "linux")]
fn lowered(pid: u32) -> bool { (unsafe { libc::getpriority(libc::PRIO_PROCESS, pid as libc::id_t) }) == 19 }

/// The process's priority once it has had time to set it, then the process stopped.
fn settled(mut child: Child) -> bool {
    let start = Instant::now();
    let mut seen = lowered(child.id());
    while !seen && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
        seen = lowered(child.id());
    }
    // A TERM rather than a kill: `serve` unlinks its socket on the way out, and a killed one
    // would leave the file behind in the real cache directory.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    let _ = child.wait();
    seen
}

#[test]
fn the_flag_a_detached_refresh_is_started_with_lowers_the_process() {
    let dir = built();
    // `--background` on a command that would otherwise run in the foreground: the flag, not the
    // command, is what a detached `update` or `model` is lowered by. `serve` is the command here
    // only because it stays up long enough to be read.
    assert!(settled(start(dir.path(), &["--background", "serve", "--every", "3600", "--idle", "60"])));
}
