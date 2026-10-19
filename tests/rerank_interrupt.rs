//! A Ctrl-C during `ask --rerank` reaches the model command too. The command runs in a process
//! group of its own so that its deadline can stop everything it started, which takes it out of
//! the terminal's group; without a forward, `zerograph` would die and the paid call run on.
#![cfg(unix)]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn an_interrupt_during_a_rerank_stops_the_model_command_too() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join("docs")).unwrap();
    std::fs::write(repo.join("docs/pay.md"), "**FR-PAY-1 · MUST · Cancellation fee**\n\nThe fee is charged on its own.\n").unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let pidfile = dir.path().join("pid");
    // `exec`, so the pid written is the process that would go on spending.
    std::fs::write(bin.join("claude"), format!("#!/bin/sh\necho $$ > '{}'\nexec sleep 30\n", pidfile.display())).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap()))).unwrap();
    let zerograph = || {
        let mut c = Command::new(env!("CARGO_BIN_EXE_zerograph"));
        c.env("PATH", &path).env("ZEROGRAPH_CONFIG", dir.path().join("no-machine-config.toml"))
            .env_remove("ZEROGRAPH_CODE_GLOBS")
            .arg("--no-dense").arg("--repo").arg(&repo);
        c
    };
    assert!(zerograph().arg("build").output().unwrap().status.success());

    let mut asking = zerograph().args(["ask", "--no-serve", "--rerank", "fee"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let started = Instant::now();
    let model: i32 = loop {
        if let Some(pid) = std::fs::read_to_string(&pidfile).ok().and_then(|s| s.trim().parse().ok()) { break pid; }
        assert!(started.elapsed() < Duration::from_secs(30), "the model command never started");
        std::thread::sleep(Duration::from_millis(20));
    };
    // SAFETY: signals to processes this test started.
    unsafe { libc::kill(asking.id() as i32, libc::SIGINT); }
    let status = asking.wait().unwrap();
    assert!(!status.success(), "an interrupted ask does not report success");
    let gone = (0..200).any(|_| {
        // SAFETY: signal 0 only asks whether the pid exists.
        let alive = unsafe { libc::kill(model, 0) } == 0;
        if alive { std::thread::sleep(Duration::from_millis(10)); }
        !alive
    });
    if !gone { unsafe { libc::kill(model, libc::SIGKILL); } }
    assert!(gone, "the model command {model} outlived the interrupt");
}
