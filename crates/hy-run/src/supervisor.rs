//! The run loop.
//!
//! The server never restarts itself: it exits with code 8 to ask for one. Every other exit
//! code ends the loop, matching `start.sh`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use hy_instance::Instance;
use tokio::process::Command;

use crate::error::Result;
use crate::lock::RunLock;
use crate::signal::{self, Shutdown};
use crate::console::{self, Console};
use crate::output::{Output, OutputSink, StopHandle};
use crate::{command, staging};

/// The server asks to be restarted by exiting with this code.
pub const RESTART_EXIT_CODE: i32 = 8;

/// Below this, a non-zero exit right after an update looks like the update broke it.
const SUSPECT_CRASH_WINDOW: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub java: PathBuf,
    pub server_args: Vec<String>,
    /// Forward this process's stdin to the server. Off when nothing is typing at it — a
    /// systemd unit — and when a UI supplies input through [`Console::send`] instead.
    pub forward_stdin: bool,
    pub output: Output,
    /// Lets a UI request the stop that a signal would otherwise deliver.
    pub stop: StopHandle,
    /// Supply one to share it with a UI, which needs to send commands to whichever server
    /// is currently running. Left unset, the supervisor makes its own.
    pub console: Option<Console>,
}

impl RunOptions {
    pub fn new(java: PathBuf) -> Self {
        Self {
            java,
            server_args: Vec::new(),
            forward_stdin: false,
            output: Output::Inherit,
            stop: StopHandle::new(),
            console: None,
        }
    }
}

/// How long the server gets to act on `shutdown` before the signal is tried as well.
const CONSOLE_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct Outcome {
    /// The server's own exit code, whatever it was.
    pub code: i32,
    pub restarts: u32,
    /// The operator asked for the stop, so the exit code describes how the server was
    /// interrupted rather than whether anything went wrong.
    pub stopped_by_request: bool,
    /// Exited non-zero within [`SUSPECT_CRASH_WINDOW`] of an update being applied.
    pub suspect_update: bool,
}

/// Progress callbacks, so this crate never prints.
pub trait RunReporter: Sync {
    fn starting(&self, _attempt: u32, _command: &crate::ServerCommand) {}
    fn applied_update(&self) {}
    fn restarting(&self) {}
    fn stopping(&self) {}
    fn killing(&self) {}
}

pub struct NoReporter;
impl RunReporter for NoReporter {}

pub async fn run(
    instance: &Instance,
    options: &RunOptions,
    reporter: &dyn RunReporter,
) -> Result<Outcome> {
    let layout = instance.layout();
    let _lock = RunLock::acquire(layout.root())?;

    let mut shutdown = Shutdown::new()?;
    let console = options
        .console
        .clone()
        .unwrap_or_else(|| Console::new(options.forward_stdin));
    let mut restarts = 0;

    loop {
        let applied = staging::apply(layout)?;
        if applied {
            reporter.applied_update();
        }

        let spec = command::build(instance, &options.java, &options.server_args)?;
        reporter.starting(restarts + 1, &spec);

        let started = Instant::now();
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&spec.working_dir)
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true);
        if options.output.is_captured() {
            command
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
        }
        let mut child = command.spawn()?;

        if let Some(stdin) = child.stdin.take() {
            console.attach(stdin).await;
        }
        if let Output::To(sink) = &options.output {
            capture(&mut child, sink.clone());
        }

        let (status, requested) =
            wait(&mut child, &mut shutdown, &options.stop, &console, reporter).await?;
        console.detach().await;
        let elapsed = started.elapsed();
        let code = signal::exit_code(status);

        // A deliberate stop wins over a restart request: an update that lands exactly as
        // the operator hits Ctrl-C must not silently bring the server back up.
        if !requested && code == RESTART_EXIT_CODE {
            reporter.restarting();
            restarts += 1;
            continue;
        }

        return Ok(Outcome {
            code,
            restarts,
            stopped_by_request: requested,
            suspect_update: !requested
                && applied
                && code != 0
                && elapsed < SUSPECT_CRASH_WINDOW,
        });
    }
}

/// Both streams feed one sink, in the order the server produced them.
fn capture(child: &mut tokio::process::Child, sink: std::sync::Arc<dyn OutputSink>) {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let mut readers: Vec<std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>> = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        readers.push(Box::pin(stdout));
    }
    if let Some(stderr) = child.stderr.take() {
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

/// Waits for the child, a shutdown signal, or a stop asked for by a UI. The bool reports
/// whether the stop was requested rather than the server ending on its own.
async fn wait(
    child: &mut tokio::process::Child,
    shutdown: &mut Shutdown,
    stop: &StopHandle,
    console: &Console,
    reporter: &dyn RunReporter,
) -> Result<(std::process::ExitStatus, bool)> {
    tokio::select! {
        status = child.wait() => Ok((status?, false)),
        // In raw mode a UI gets Ctrl-C as a key event, never as a signal, so both sources
        // have to lead to the same place.
        _ = async { tokio::select! { _ = shutdown.recv() => {}, _ = stop.requested() => {} } } => {
            reporter.stopping();

            // The console command is the portable route and the only one that works where
            // the terminal never raises a control event. The signal is the belt to its
            // braces, and covers a server whose console is not reading.
            if console.send(console::SHUTDOWN_COMMAND).await
                && let Ok(status) =
                    tokio::time::timeout(CONSOLE_SHUTDOWN_GRACE, child.wait()).await
            {
                return Ok((status?, true));
            }
            signal::request_stop(child);

            // Returning here would hand the shell back while the world is still being
            // written. A second signal is the operator insisting.
            tokio::select! {
                status = child.wait() => Ok((status?, true)),
                _ = async { tokio::select! {
                    _ = shutdown.recv() => {},
                    _ = stop.requested() => {},
                } } => {
                    reporter.killing();
                    child.kill().await?;
                    Ok((child.wait().await?, true))
                }
            }
        }
    }
}
