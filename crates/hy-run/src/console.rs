//! Ownership of the server's console input.
//!
//! `hy` holds the child's stdin rather than letting it inherit ours, for one load-bearing
//! reason: a Ctrl-C typed into a terminal that does not raise a console control event —
//! Git Bash's mintty, notably — arrives at the JVM as a literal `0x03` byte instead. jline
//! reads that in raw mode, turns it into an interrupt, and the server's reader thread ends
//! silently. The server keeps running and never accepts another command, so there is no
//! way left to stop it short of a kill.
//!
//! Holding the pipe means `0x03` never reaches the child, and `hy` can issue the server's
//! own `shutdown` command itself, which works the same on every platform.
//!
//! The writer is rebound on each restart of the exit-8 loop, while the input source stays
//! put — there is only one terminal to read, and two readers would race for keystrokes.

use std::sync::Arc;

use tokio::io::AsyncWriteExt;
use tokio::process::ChildStdin;
use tokio::sync::Mutex;

/// The server's own graceful-shutdown command. Undocumented in the manual, which lists
/// `/update` and `/auth` but not this; `stop` and `exit` are *not* accepted.
pub const SHUTDOWN_COMMAND: &str = "shutdown";

const CTRL_C: u8 = 0x03;

/// A handle to whichever server process is currently running.
#[derive(Clone)]
pub struct Console {
    target: Arc<Mutex<Option<ChildStdin>>>,
}

impl Console {
    /// `forward_terminal` reads this process's stdin into the child. The TUI will supply
    /// input through [`Console::send`] instead, and pass `false` here.
    pub fn new(forward_terminal: bool) -> Self {
        let console = Self {
            target: Arc::new(Mutex::new(None)),
        };
        if forward_terminal {
            console.clone().read_terminal();
        }
        console
    }

    /// Point the console at a newly spawned server.
    pub async fn attach(&self, stdin: ChildStdin) {
        *self.target.lock().await = Some(stdin);
    }

    /// Drop the pipe, so the server sees EOF on stdin.
    pub async fn detach(&self) {
        self.target.lock().await.take();
    }

    /// Send one command, newline-terminated. Returns whether it reached a running server.
    pub async fn send(&self, command: &str) -> bool {
        self.write(format!("{command}\n").as_bytes()).await
    }

    async fn write(&self, bytes: &[u8]) -> bool {
        let mut target = self.target.lock().await;
        let Some(stdin) = target.as_mut() else {
            return false;
        };
        if stdin.write_all(bytes).await.is_err() {
            return false;
        }
        stdin.flush().await.is_ok()
    }

    fn read_terminal(self) {
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<Vec<u8>>(64);

        // A plain OS thread rather than `tokio::io::stdin`, which reads on the blocking
        // pool — and dropping the runtime waits for those. A read parked on a terminal
        // nobody is typing at would then keep `hy` alive after the server had exited,
        // until the operator pressed a key to release it. Detached threads are not waited
        // for, so the process leaves when it is done.
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut buffer = [0u8; 1024];
            loop {
                let read = match std::io::Read::read(&mut stdin, &mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                let filtered = strip_interrupts(&buffer[..read]);
                if filtered.is_empty() {
                    continue;
                }
                if sender.blocking_send(filtered).is_err() {
                    break;
                }
            }
        });

        tokio::spawn(async move {
            while let Some(bytes) = receiver.recv().await {
                self.write(&bytes).await;
            }
        });
    }
}

/// Drop `0x03` from a keystroke stream.
///
/// Terminals that raise a proper control event never put it in the stream, so this is a
/// no-op there; the ones that do would otherwise kill the server's console reader.
fn strip_interrupts(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().copied().filter(|b| *b != CTRL_C).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupts_are_stripped_and_everything_else_survives() {
        assert_eq!(strip_interrupts(b"shutdown\n"), b"shutdown\n");
        assert_eq!(strip_interrupts(&[b'a', CTRL_C, b'b']), b"ab");
        assert!(strip_interrupts(&[CTRL_C]).is_empty());
        // Other control bytes are the server's business, not ours.
        assert_eq!(strip_interrupts(&[0x04, 0x1b]), vec![0x04, 0x1b]);
    }

    #[tokio::test]
    async fn sending_without_a_server_is_not_an_error() {
        let console = Console::new(false);
        assert!(!console.send(SHUTDOWN_COMMAND).await);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn commands_reach_the_attached_process() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Stdio;

        let dir = tempfile::tempdir().unwrap();
        let seen = dir.path().join("seen");
        let script = dir.path().join("reader.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nwhile read line; do echo \"$line\" >> \"{}\"; done\n",
                seen.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut child = tokio::process::Command::new(&script)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();

        let console = Console::new(false);
        console.attach(child.stdin.take().unwrap()).await;
        assert!(console.send(SHUTDOWN_COMMAND).await);

        // EOF ends the reader loop, so the child exits and flushes.
        console.detach().await;
        child.wait().await.unwrap();

        assert_eq!(std::fs::read_to_string(&seen).unwrap().trim(), "shutdown");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_restart_rebinds_to_the_new_process() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Stdio;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("reader.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nwhile read line; do echo \"$line\" > \"$1\"; done\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let console = Console::new(false);
        let mut outputs = Vec::new();

        for cycle in 0..2 {
            let seen = dir.path().join(format!("cycle{cycle}"));
            let mut child = tokio::process::Command::new(&script)
                .arg(&seen)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()
                .unwrap();

            console.attach(child.stdin.take().unwrap()).await;
            assert!(console.send("hello").await, "cycle {cycle}");
            console.detach().await;
            child.wait().await.unwrap();
            outputs.push(std::fs::read_to_string(&seen).unwrap().trim().to_string());
        }

        assert_eq!(outputs, ["hello", "hello"]);
    }
}
