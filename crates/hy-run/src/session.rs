//! Driving a server process over stdin, reading its console output.
//!
//! Used by the bootstrap install, where `hy` has to type `/auth login device` and watch for
//! a device code. Ordinary `hy run` does not use this — it inherits stdio instead, so no
//! output passes through us.

use std::ffi::OsString;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

use crate::error::Result;

pub struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
}

impl Session {
    /// Output is colourised even when piped, so `NO_COLOR` is set to keep the stream
    /// parseable. Callers still strip escapes, since honouring it is not guaranteed.
    pub fn spawn(program: &Path, args: &[OsString], working_dir: &Path) -> Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .current_dir(working_dir)
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        let stdin = child.stdin.take();
        let (sender, lines) = mpsc::channel(256);

        // Both streams feed one channel: the server interleaves progress and prompts across
        // them, and reading only stdout would miss half the flow.
        if let Some(stdout) = child.stdout.take() {
            forward(stdout, sender.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            forward(stderr, sender);
        }

        Ok(Self {
            child,
            stdin,
            lines,
        })
    }

    pub async fn send(&mut self, command: &str) -> Result<()> {
        if let Some(stdin) = self.stdin.as_mut() {
            stdin.write_all(command.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await?;
        }
        Ok(())
    }

    /// The next console line, or `None` once both streams have closed.
    pub async fn next_line(&mut self) -> Option<String> {
        self.lines.recv().await
    }

    /// `None` on timeout, `Some(None)` at end of output — a quiet server and a finished one
    /// need different handling.
    pub async fn next_line_within(&mut self, limit: Duration) -> Option<Option<String>> {
        tokio::time::timeout(limit, self.lines.recv()).await.ok()
    }

    /// Close stdin so the server sees EOF, then wait for it to exit.
    pub async fn finish(mut self) -> Result<ExitStatus> {
        self.stdin.take();
        Ok(self.child.wait().await?)
    }

    pub async fn kill(&mut self) -> Result<()> {
        self.child.kill().await?;
        Ok(())
    }
}

fn forward<R>(reader: R, sender: mpsc::Sender<String>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if sender.send(line).await.is_err() {
                break;
            }
        }
    });
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_both_streams_and_writes_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("echo.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\necho to-stdout\necho to-stderr >&2\nread line\necho \"got $line\"\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let mut session = Session::spawn(&script, &[], dir.path()).unwrap();
        let mut seen = Vec::new();
        while seen.len() < 2 {
            seen.push(session.next_line().await.unwrap());
        }
        assert!(seen.contains(&"to-stdout".to_string()));
        assert!(seen.contains(&"to-stderr".to_string()));

        session.send("hello").await.unwrap();
        assert_eq!(session.next_line().await.as_deref(), Some("got hello"));
        assert!(session.finish().await.unwrap().success());
    }

    #[tokio::test]
    async fn a_quiet_server_times_out_rather_than_hanging() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("quiet.sh");
        std::fs::write(&script, "#!/bin/sh\nsleep 5\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let mut session = Session::spawn(&script, &[], dir.path()).unwrap();
        assert!(
            session
                .next_line_within(Duration::from_millis(200))
                .await
                .is_none()
        );
        session.kill().await.unwrap();
    }
}
