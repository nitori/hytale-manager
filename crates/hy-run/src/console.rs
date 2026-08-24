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

use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;

/// The server's own graceful-shutdown command. Undocumented in the manual, which lists
/// `/update` and `/auth` but not this; `stop` and `exit` are *not* accepted.
pub const SHUTDOWN_COMMAND: &str = "shutdown";

const CTRL_C: u8 = 0x03;

/// The server's console input. Boxed rather than a `ChildStdin` so that nothing here has to
/// know a process is on the other end.
pub type ConsoleWriter = Box<dyn AsyncWrite + Send + Unpin>;

/// A handle to whichever server process is currently running.
#[derive(Clone, Default)]
pub struct Console {
    target: Arc<Mutex<Option<ConsoleWriter>>>,
}

impl std::fmt::Debug for Console {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Console")
    }
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
    pub async fn attach(&self, writer: ConsoleWriter) {
        *self.target.lock().await = Some(writer);
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

    /// An in-memory stand-in for the server's stdin, and what was written to it.
    fn pipe() -> (ConsoleWriter, Arc<std::sync::Mutex<Vec<u8>>>) {
        use std::pin::Pin;
        use std::task::{Context, Poll};

        struct Sink(Arc<std::sync::Mutex<Vec<u8>>>);

        impl AsyncWrite for Sink {
            fn poll_write(
                self: Pin<&mut Self>,
                _: &mut Context<'_>,
                buf: &[u8],
            ) -> Poll<std::io::Result<usize>> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Poll::Ready(Ok(buf.len()))
            }

            fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
                Poll::Ready(Ok(()))
            }

            fn poll_shutdown(
                self: Pin<&mut Self>,
                _: &mut Context<'_>,
            ) -> Poll<std::io::Result<()>> {
                Poll::Ready(Ok(()))
            }
        }

        let written = Arc::new(std::sync::Mutex::new(Vec::new()));
        (Box::new(Sink(written.clone())), written)
    }

    fn text(written: &Arc<std::sync::Mutex<Vec<u8>>>) -> String {
        String::from_utf8(written.lock().unwrap().clone()).unwrap()
    }

    #[tokio::test]
    async fn commands_reach_the_attached_server() {
        let (writer, written) = pipe();
        let console = Console::new(false);

        console.attach(writer).await;
        assert!(console.send(SHUTDOWN_COMMAND).await);

        // Newline-terminated, or the server's console reader never sees a complete line.
        assert_eq!(text(&written), "shutdown\n");
    }

    #[tokio::test]
    async fn a_restart_rebinds_to_the_new_server() {
        let console = Console::new(false);
        let (first, first_written) = pipe();
        let (second, second_written) = pipe();

        console.attach(first).await;
        assert!(console.send("hello").await);
        console.detach().await;

        console.attach(second).await;
        assert!(console.send("again").await);

        assert_eq!(text(&first_written), "hello\n");
        assert_eq!(
            text(&second_written),
            "again\n",
            "the rebind must take effect"
        );
    }

    #[tokio::test]
    async fn a_detached_console_reports_that_nothing_received_it() {
        let (writer, written) = pipe();
        let console = Console::new(false);

        console.attach(writer).await;
        console.detach().await;

        assert!(!console.send(SHUTDOWN_COMMAND).await);
        assert_eq!(
            text(&written),
            "",
            "a detached server must not be written to"
        );
    }
}
