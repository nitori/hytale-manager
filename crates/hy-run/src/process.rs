//! The seam between the run loop and an actual operating-system process.
//!
//! Everything that touches the OS lives behind [`Launcher`] and [`ServerProcess`], so the
//! loop above — exit-8 restarts, staging order, the shutdown escalation — is ordinary logic
//! over a trait and can be driven by a fake.
//!
//! [`ServerProcess::wait`] yields an exit *code* rather than an `ExitStatus` deliberately:
//! `ExitStatus` has no portable constructor, so a fake could not produce one.

use std::future::Future;
use std::sync::Arc;

use crate::command::ServerCommand;
use crate::console::ConsoleWriter;
use crate::error::Result;
use crate::output::OutputSink;
use crate::signal;

/// One running server, from spawn to exit.
pub trait ServerProcess: Send {
    /// The pipe to the server's console. Available once.
    fn console(&mut self) -> Option<ConsoleWriter>;

    /// Start forwarding the server's output to `sink`.
    fn capture(&mut self, sink: Arc<dyn OutputSink>);

    /// Ask the OS to stop it — the fallback for a server whose console is not reading.
    fn request_stop(&mut self);

    /// The exit code a shell would report. Callable more than once: the shutdown path waits,
    /// escalates, and waits again.
    fn wait(&mut self) -> impl Future<Output = Result<i32>> + Send;

    fn kill(&mut self) -> impl Future<Output = Result<()>> + Send;
}

/// How a built command becomes a running server.
pub trait Launcher: Sync {
    type Process: ServerProcess;

    fn spawn(&self, command: &ServerCommand, capture_output: bool) -> Result<Self::Process>;
}

/// Spawns a real JVM.
pub struct SystemLauncher;

impl Launcher for SystemLauncher {
    type Process = ChildProcess;

    fn spawn(&self, spec: &ServerCommand, capture_output: bool) -> Result<ChildProcess> {
        let mut command = tokio::process::Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&spec.working_dir)
            .envs(spec.env.iter().map(|(k, v)| (k, v)))
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true);
        if capture_output {
            command
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
        }
        Ok(ChildProcess {
            child: command.spawn()?,
        })
    }
}

pub struct ChildProcess {
    child: tokio::process::Child,
}

impl ServerProcess for ChildProcess {
    fn console(&mut self) -> Option<ConsoleWriter> {
        self.child
            .stdin
            .take()
            .map(|stdin| Box::new(stdin) as ConsoleWriter)
    }

    /// Both streams feed one sink, in the order the server produced them.
    fn capture(&mut self, sink: Arc<dyn OutputSink>) {
        use tokio::io::{AsyncBufReadExt, BufReader};

        let mut readers: Vec<std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>> = Vec::new();
        if let Some(stdout) = self.child.stdout.take() {
            readers.push(Box::pin(stdout));
        }
        if let Some(stderr) = self.child.stderr.take() {
            readers.push(Box::pin(stderr));
        }

        for reader in readers {
            let sink = sink.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(reader).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    sink.line(line);
                }
            });
        }
    }

    fn request_stop(&mut self) {
        signal::request_stop(&mut self.child);
    }

    async fn wait(&mut self) -> Result<i32> {
        Ok(signal::exit_code(self.child.wait().await?))
    }

    async fn kill(&mut self) -> Result<()> {
        self.child.kill().await?;
        Ok(())
    }
}
