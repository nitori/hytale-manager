//! Shutdown signals, and delivering them onward to the JVM.

use tokio::process::Child;

/// SIGINT and SIGTERM on unix; Ctrl-C on Windows.
pub struct Shutdown {
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
}

impl Shutdown {
    #[cfg(unix)]
    pub fn new() -> std::io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }

    #[cfg(not(unix))]
    pub fn new() -> std::io::Result<Self> {
        Ok(Self {})
    }

    #[cfg(unix)]
    pub async fn recv(&mut self) {
        tokio::select! {
            _ = self.interrupt.recv() => {}
            _ = self.terminate.recv() => {}
        }
    }

    #[cfg(not(unix))]
    pub async fn recv(&mut self) {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Ask the server to stop, giving it the chance to save.
///
/// A terminal delivers Ctrl-C to the whole process group, so the JVM has usually seen it
/// already; this covers `kill` aimed at `hy` alone, and systemd's `KillMode=mixed`.
/// Windows has no SIGTERM equivalent, so the JVM's shutdown hooks do not run there.
pub fn request_stop(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // Safety: `kill` with a pid we own and a valid signal number.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        }
    }

    #[cfg(not(unix))]
    {
        let _ = child.start_kill();
    }
}

/// The exit code a shell would report, so `hy run` propagates what `start.sh` did.
pub fn exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    -1
}
