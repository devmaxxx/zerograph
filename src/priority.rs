//! Background priority for the writes zerograph starts on its own: the detached `update` a reader,
//! the session hook or the MCP server hands a refresh to, and `watch`. Nobody waits on either, and a
//! build, a test run or an editor running beside them should not wait because of them. A command
//! someone typed and is waiting on — `ask`, `build`, `update` without `--detach`, `embed`, `model`
//! — keeps the priority it was started with, and so does `serve`, whose poll a question can be
//! waiting behind.
//!
//! This is not the band removed on 2026-09-09 (docs/history.md): that one took every rebuild,
//! foreground ones included, and four times the wall was paid by whoever was waiting on it.
//!
//! Failing to lower is never an error: the work is the same at normal priority, so the reason
//! goes to the run's stderr — `background.log` for a detached refresh — and the work goes on.

/// Lowers the whole process. Called first thing in `run`, before the rayon pool or an ONNX session
/// exists: on Linux priority belongs to a thread, and a thread takes its creator's when it is
/// spawned, so the pools started later run lowered too.
pub(crate) fn background_process() {
    match lower() {
        Lowered::All => {}
        Lowered::Cpu(e) => eprintln!("priority: CPU lowered, I/O class not ({e})"),
        Lowered::None(e) => eprintln!("priority: not lowered ({e}); running at normal priority"),
    }
}

enum Lowered { All, #[allow(dead_code)] Cpu(std::io::Error), None(std::io::Error) }

/// Darwin's background band: the scheduler runs the work only on time nothing else wants, and its
/// disk and network I/O are throttled behind everyone else's.
#[cfg(target_os = "macos")]
fn lower() -> Lowered {
    match unsafe { libc::setpriority(libc::PRIO_DARWIN_PROCESS, 0, libc::PRIO_DARWIN_BG) } {
        0 => Lowered::All,
        _ => Lowered::None(std::io::Error::last_os_error()),
    }
}

/// Nice 19 and the idle I/O class. On Linux `who = 0` names the calling thread, not the process,
/// which is why this runs before any other thread exists.
#[cfg(target_os = "linux")]
fn lower() -> Lowered {
    // linux/ioprio.h; libc carries the syscall number but not these.
    const IOPRIO_WHO_PROCESS: libc::c_int = 1;
    const IOPRIO_CLASS_IDLE: libc::c_int = 3;
    const IOPRIO_CLASS_SHIFT: libc::c_int = 13;
    if unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 19) } != 0 {
        return Lowered::None(std::io::Error::last_os_error());
    }
    match unsafe { libc::syscall(libc::SYS_ioprio_set, IOPRIO_WHO_PROCESS, 0, IOPRIO_CLASS_IDLE << IOPRIO_CLASS_SHIFT) } {
        0 => Lowered::All,
        _ => Lowered::Cpu(std::io::Error::last_os_error()),
    }
}

/// Background processing mode: low CPU, I/O and memory priority together. It can only be asked
/// for the current process, which is the one asking.
#[cfg(windows)]
fn lower() -> Lowered {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetPriorityClass, PROCESS_MODE_BACKGROUND_BEGIN};
    match unsafe { SetPriorityClass(GetCurrentProcess(), PROCESS_MODE_BACKGROUND_BEGIN) } {
        0 => Lowered::None(std::io::Error::last_os_error()),
        _ => Lowered::All,
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn lower() -> Lowered {
    Lowered::None(std::io::Error::new(std::io::ErrorKind::Unsupported, "no background priority on this platform"))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn current() -> (i32, i32) {
        const IOPRIO_WHO_PROCESS: libc::c_int = 1;
        let nice = unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) };
        let io = unsafe { libc::syscall(libc::SYS_ioprio_get, IOPRIO_WHO_PROCESS, 0) } as i32;
        (nice, io >> 13)
    }

    // Linux only, where `who = 0` is the calling thread: on a thread of its own the test binary's
    // other threads keep running at normal priority. Elsewhere the call takes the whole process,
    // and `tests/background.rs` reads it from outside one.
    #[test]
    fn nice_19_and_the_idle_io_class_and_the_thread_beside_it_keeps_its_own() {
        let before = current();
        let lowered = std::thread::spawn(|| { assert!(matches!(lower(), Lowered::All)); current() }).join().unwrap();
        assert_eq!(lowered, (19, 3));
        assert_eq!(current(), before, "the calling thread kept its priority");
    }
}
