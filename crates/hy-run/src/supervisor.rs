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
    /// systemd unit — and later when a UI supplies input instead.
    pub forward_stdin: bool,
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
    let console = Console::new(options.forward_stdin);
    let mut restarts = 0;

    loop {
        let applied = staging::apply(layout)?;
        if applied {
            reporter.applied_update();
        }

        let spec = command::build(instance, &options.java, &options.server_args)?;
        reporter.starting(restarts + 1, &spec);

        let started = Instant::now();
        let mut child = Command::new(&spec.program)
            .args(&spec.args)
            .current_dir(&spec.working_dir)
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        if let Some(stdin) = child.stdin.take() {
            console.attach(stdin).await;
        }

        let (status, requested) = wait(&mut child, &mut shutdown, &console, reporter).await?;
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

/// Waits for the child, or for a shutdown signal. The bool reports whether a stop was
/// requested.
async fn wait(
    child: &mut tokio::process::Child,
    shutdown: &mut Shutdown,
    console: &Console,
    reporter: &dyn RunReporter,
) -> Result<(std::process::ExitStatus, bool)> {
    tokio::select! {
        status = child.wait() => Ok((status?, false)),
        _ = shutdown.recv() => {
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
                _ = shutdown.recv() => {
                    reporter.killing();
                    child.kill().await?;
                    Ok((child.wait().await?, true))
                }
            }
        }
    }
}
