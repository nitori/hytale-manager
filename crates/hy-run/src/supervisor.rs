//! The run loop.
//!
//! The server never restarts itself: it exits with code 8 to ask for one. Every other exit
//! code ends the loop, matching `start.sh`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use hy_instance::Instance;

use crate::console::{self, Console};
use crate::error::Result;
use crate::lock::RunLock;
use crate::output::{Output, StopHandle};
use crate::process::{Launcher, ServerProcess, SystemLauncher};
use crate::signal::Shutdown;
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
    run_with(instance, options, reporter, &SystemLauncher).await
}

async fn run_with<L: Launcher>(
    instance: &Instance,
    options: &RunOptions,
    reporter: &dyn RunReporter,
    launcher: &L,
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
        let mut server = launcher.spawn(&spec, options.output.is_captured())?;

        if let Some(writer) = server.console() {
            console.attach(writer).await;
        }
        if let Output::To(sink) = &options.output {
            server.capture(sink.clone());
        }

        let (code, requested) = wait(
            &mut server,
            &mut shutdown,
            &options.stop,
            &console,
            reporter,
        )
        .await?;
        console.detach().await;
        let elapsed = started.elapsed();

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
            suspect_update: !requested && applied && code != 0 && elapsed < SUSPECT_CRASH_WINDOW,
        });
    }
}

/// Waits for the server, a shutdown signal, or a stop asked for by a UI. The bool reports
/// whether the stop was requested rather than the server ending on its own.
async fn wait<P: ServerProcess>(
    server: &mut P,
    shutdown: &mut Shutdown,
    stop: &StopHandle,
    console: &Console,
    reporter: &dyn RunReporter,
) -> Result<(i32, bool)> {
    tokio::select! {
        code = server.wait() => Ok((code?, false)),
        // In raw mode a UI gets Ctrl-C as a key event, never as a signal, so both sources
        // have to lead to the same place.
        _ = async { tokio::select! { _ = shutdown.recv() => {}, _ = stop.requested() => {} } } => {
            reporter.stopping();

            // The console command is the portable route and the only one that works where
            // the terminal never raises a control event. The signal is the belt to its
            // braces, and covers a server whose console is not reading.
            if console.send(console::SHUTDOWN_COMMAND).await
                && let Ok(code) =
                    tokio::time::timeout(CONSOLE_SHUTDOWN_GRACE, server.wait()).await
            {
                return Ok((code?, true));
            }
            server.request_stop();

            // Returning here would hand the shell back while the world is still being
            // written. A second signal is the operator insisting.
            tokio::select! {
                code = server.wait() => Ok((code?, true)),
                _ = async { tokio::select! {
                    _ = shutdown.recv() => {},
                    _ = stop.requested() => {},
                } } => {
                    reporter.killing();
                    server.kill().await?;
                    Ok((server.wait().await?, true))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::ServerCommand;
    use crate::console::ConsoleWriter;
    use crate::error::Error;
    use crate::output::OutputSink;
    use crate::process::{Launcher, ServerProcess};
    use std::path::Path;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};
    use tokio::sync::watch;

    /// What it takes to make a fake server exit.
    #[derive(Clone, Copy, PartialEq)]
    enum Stops {
        /// Ends on its own, without being asked — the ordinary case.
        OnItsOwn,
        /// Acts on the `shutdown` command, as a healthy server does.
        OnTheConsoleCommand,
        /// Console reader is dead, so only the signal gets through.
        OnlyOnASignal,
        /// Ignores both, leaving nothing but a kill.
        NotAtAll,
    }

    /// What the supervisor did, across every attempt.
    #[derive(Default)]
    struct Log {
        commands: Vec<ServerCommand>,
        /// The jar as it stood when each attempt spawned. This is what proves a staged
        /// update landed *before* the server started rather than after it.
        jar_at_spawn: Vec<Vec<u8>>,
        console: Vec<String>,
        stops_requested: usize,
        kills: usize,
    }

    struct FakeLauncher {
        /// Exit code per attempt; the last one repeats.
        codes: Vec<i32>,
        stops: Stops,
        /// Poked when the fake is signalled, standing in for an operator who insists.
        stop_again: Option<StopHandle>,
        jar: std::path::PathBuf,
        log: Arc<Mutex<Log>>,
    }

    impl FakeLauncher {
        fn new(root: &Path, codes: &[i32]) -> Self {
            Self {
                codes: codes.to_vec(),
                stops: Stops::OnItsOwn,
                stop_again: None,
                jar: root.join("Server/HytaleServer.jar"),
                log: Arc::new(Mutex::new(Log::default())),
            }
        }

        fn stopping(mut self, stops: Stops) -> Self {
            self.stops = stops;
            self
        }

        fn insisting_with(mut self, stop: &StopHandle) -> Self {
            self.stop_again = Some(stop.clone());
            self
        }

        fn log(&self) -> std::sync::MutexGuard<'_, Log> {
            self.log.lock().unwrap()
        }
    }

    impl Launcher for FakeLauncher {
        type Process = FakeProcess;

        fn spawn(&self, command: &ServerCommand, _capture: bool) -> Result<FakeProcess> {
            let mut log = self.log.lock().unwrap();
            let attempt = log.commands.len();
            log.commands.push(command.clone());
            log.jar_at_spawn
                .push(std::fs::read(&self.jar).unwrap_or_default());
            drop(log);

            let code = *self.codes.get(attempt).or(self.codes.last()).unwrap_or(&0);
            let already_exited = (self.stops == Stops::OnItsOwn).then_some(code);

            let (exited, wait_for) = watch::channel(already_exited);
            Ok(FakeProcess {
                code,
                stops: self.stops,
                stop_again: self.stop_again.clone(),
                exited: Arc::new(exited),
                wait_for,
                log: self.log.clone(),
                console_taken: false,
            })
        }
    }

    struct FakeProcess {
        code: i32,
        stops: Stops,
        stop_again: Option<StopHandle>,
        exited: Arc<watch::Sender<Option<i32>>>,
        wait_for: watch::Receiver<Option<i32>>,
        log: Arc<Mutex<Log>>,
        console_taken: bool,
    }

    impl ServerProcess for FakeProcess {
        fn console(&mut self) -> Option<ConsoleWriter> {
            if std::mem::replace(&mut self.console_taken, true) {
                return None;
            }
            Some(Box::new(ConsolePipe {
                exit_on_shutdown: (self.stops == Stops::OnTheConsoleCommand).then_some(self.code),
                exited: self.exited.clone(),
                log: self.log.clone(),
                partial: Vec::new(),
            }))
        }

        fn capture(&mut self, _sink: Arc<dyn OutputSink>) {}

        fn request_stop(&mut self) {
            self.log.lock().unwrap().stops_requested += 1;
            if self.stops == Stops::OnlyOnASignal {
                let _ = self.exited.send(Some(self.code));
            } else if let Some(stop) = &self.stop_again {
                stop.stop();
            }
        }

        async fn wait(&mut self) -> Result<i32> {
            loop {
                if let Some(code) = *self.wait_for.borrow_and_update() {
                    return Ok(code);
                }
                if self.wait_for.changed().await.is_err() {
                    return Ok(self.code);
                }
            }
        }

        async fn kill(&mut self) -> Result<()> {
            self.log.lock().unwrap().kills += 1;
            let _ = self.exited.send(Some(self.code));
            Ok(())
        }
    }

    /// The fake server's stdin: records whole lines, and ends the process on `shutdown` if
    /// this one is meant to be listening.
    struct ConsolePipe {
        exit_on_shutdown: Option<i32>,
        exited: Arc<watch::Sender<Option<i32>>>,
        log: Arc<Mutex<Log>>,
        partial: Vec<u8>,
    }

    impl tokio::io::AsyncWrite for ConsolePipe {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.partial.extend_from_slice(buf);
            while let Some(end) = self.partial.iter().position(|b| *b == b'\n') {
                let line = String::from_utf8_lossy(&self.partial[..end]).into_owned();
                self.partial.drain(..=end);
                self.log.lock().unwrap().console.push(line.clone());
                if line == console::SHUTDOWN_COMMAND
                    && let Some(code) = self.exit_on_shutdown
                {
                    let _ = self.exited.send(Some(code));
                }
            }
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    /// A `game/` layout complete enough for `command::build` to accept.
    fn instance(root: &Path) -> Instance {
        std::fs::create_dir_all(root.join("Server")).unwrap();
        std::fs::write(root.join("Assets.zip"), b"assets").unwrap();
        std::fs::write(root.join("Server/HytaleServer.jar"), b"old jar").unwrap();
        std::fs::write(root.join("start.sh"), b"").unwrap();
        std::fs::write(root.join("hytale.toml"), "[java]\nversion = \">=25\"\n").unwrap();
        Instance::at(root).unwrap()
    }

    fn stage_update(root: &Path, jar: &[u8]) {
        std::fs::create_dir_all(root.join("updater/staging/Server")).unwrap();
        std::fs::write(root.join("updater/staging/Server/HytaleServer.jar"), jar).unwrap();
    }

    fn options() -> RunOptions {
        RunOptions::new(PathBuf::from("java"))
    }

    /// A supervisor that never returns is a failure, not a reason to sit here: a fake server
    /// only ends when the loop asks it to, so a missed escalation would otherwise hang the
    /// suite instead of reporting.
    async fn supervise_with(
        instance: &Instance,
        launcher: &FakeLauncher,
        options: RunOptions,
    ) -> Result<Outcome> {
        match tokio::time::timeout(
            Duration::from_secs(30),
            run_with(instance, &options, &NoReporter, launcher),
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(_) => panic!("the supervisor never finished"),
        }
    }

    async fn supervise(instance: &Instance, launcher: &FakeLauncher) -> Result<Outcome> {
        supervise_with(instance, launcher, options()).await
    }

    /// A stop already requested before the loop reaches its wait, which is the same permit
    /// a Ctrl-C would leave behind.
    async fn supervise_stopping(
        instance: &Instance,
        launcher: &FakeLauncher,
        stop: StopHandle,
    ) -> Result<Outcome> {
        stop.stop();
        supervise_with(instance, launcher, RunOptions { stop, ..options() }).await
    }

    #[tokio::test]
    async fn exit_8_restarts_and_any_other_code_stops() {
        let dir = tempfile::tempdir().unwrap();
        let instance = instance(dir.path());
        let launcher = FakeLauncher::new(dir.path(), &[8, 8, 0]);

        let outcome = supervise(&instance, &launcher).await.unwrap();

        assert_eq!(outcome.code, 0);
        assert_eq!(outcome.restarts, 2);
        assert_eq!(launcher.log().commands.len(), 3);
    }

    #[tokio::test]
    async fn a_clean_exit_does_not_restart() {
        let dir = tempfile::tempdir().unwrap();
        let instance = instance(dir.path());
        let launcher = FakeLauncher::new(dir.path(), &[0]);

        let outcome = supervise(&instance, &launcher).await.unwrap();

        assert_eq!(outcome.restarts, 0);
        assert_eq!(launcher.log().commands.len(), 1);
    }

    #[tokio::test]
    async fn a_nonzero_exit_is_propagated() {
        let dir = tempfile::tempdir().unwrap();
        let instance = instance(dir.path());
        let launcher = FakeLauncher::new(dir.path(), &[3]);

        let outcome = supervise(&instance, &launcher).await.unwrap();

        assert_eq!(outcome.code, 3);
        assert!(!outcome.suspect_update);
    }

    #[tokio::test]
    async fn the_server_starts_from_the_server_directory() {
        let dir = tempfile::tempdir().unwrap();
        let instance = instance(dir.path());
        let launcher = FakeLauncher::new(dir.path(), &[0]);

        supervise(&instance, &launcher).await.unwrap();

        // The updater stays disabled unless the process starts from `Server/`.
        let log = launcher.log();
        assert!(log.commands[0].working_dir.ends_with("Server"));
        assert_eq!(log.commands[0].program, Path::new("java"));
    }

    #[tokio::test]
    async fn a_staged_update_is_applied_before_starting() {
        let dir = tempfile::tempdir().unwrap();
        let instance = instance(dir.path());
        stage_update(dir.path(), b"new jar");
        let launcher = FakeLauncher::new(dir.path(), &[0]);

        supervise(&instance, &launcher).await.unwrap();

        assert_eq!(
            launcher.log().jar_at_spawn[0],
            b"new jar",
            "the update must land before the server starts, not after"
        );
    }

    #[tokio::test]
    async fn a_crash_soon_after_an_update_is_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let instance = instance(dir.path());
        stage_update(dir.path(), b"broken jar");
        let launcher = FakeLauncher::new(dir.path(), &[1]);

        let outcome = supervise(&instance, &launcher).await.unwrap();

        assert_eq!(outcome.code, 1);
        assert!(outcome.suspect_update);
    }

    #[tokio::test]
    async fn a_crash_without_an_update_is_not_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let instance = instance(dir.path());
        let launcher = FakeLauncher::new(dir.path(), &[1]);

        let outcome = supervise(&instance, &launcher).await.unwrap();

        assert!(!outcome.suspect_update);
    }

    #[tokio::test]
    async fn a_restart_re_applies_staging_each_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let instance = instance(dir.path());
        stage_update(dir.path(), b"first");
        let launcher = FakeLauncher::new(dir.path(), &[8, 0]);

        supervise(&instance, &launcher).await.unwrap();

        // Staging is consumed by the first cycle, so the second must not re-apply a stale
        // copy over a jar the update itself replaced.
        let log = launcher.log();
        assert_eq!(log.jar_at_spawn, [b"first".to_vec(), b"first".to_vec()]);
        assert!(!instance.layout().staging().exists());
    }

    #[tokio::test]
    async fn a_second_run_is_refused_while_one_holds_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let instance = instance(dir.path());
        let _held = RunLock::acquire(dir.path()).unwrap();
        let launcher = FakeLauncher::new(dir.path(), &[0]);

        let error = supervise(&instance, &launcher).await.unwrap_err();

        assert!(matches!(error, Error::AlreadyRunning(_)));
        // Two JVMs on one universe/ corrupt it, so nothing should have started.
        assert!(launcher.log().commands.is_empty());
    }

    #[tokio::test]
    async fn a_missing_jar_fails_before_taking_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let instance = instance(dir.path());
        std::fs::remove_file(instance.layout().jar()).unwrap();
        let launcher = FakeLauncher::new(dir.path(), &[0]);

        let error = supervise(&instance, &launcher).await.unwrap_err();

        assert!(matches!(error, Error::MissingJar(_)));
    }

    #[tokio::test]
    async fn a_requested_stop_asks_the_server_over_its_own_console() {
        let dir = tempfile::tempdir().unwrap();
        let instance = instance(dir.path());
        let launcher = FakeLauncher::new(dir.path(), &[130]).stopping(Stops::OnTheConsoleCommand);

        let outcome = supervise_stopping(&instance, &launcher, StopHandle::new())
            .await
            .unwrap();

        let log = launcher.log();
        assert_eq!(log.console, [console::SHUTDOWN_COMMAND]);
        assert_eq!(log.stops_requested, 0, "the console was enough");
        assert_eq!(log.kills, 0);
        assert!(outcome.stopped_by_request);
        assert_eq!(outcome.code, 130, "the server's own code is still reported");
    }

    /// The mintty case: the console reader is dead, so the command goes nowhere and only
    /// the signal gets through.
    #[tokio::test(start_paused = true)]
    async fn a_server_that_ignores_the_console_is_signalled() {
        let dir = tempfile::tempdir().unwrap();
        let instance = instance(dir.path());
        let launcher = FakeLauncher::new(dir.path(), &[130]).stopping(Stops::OnlyOnASignal);

        let outcome = supervise_stopping(&instance, &launcher, StopHandle::new())
            .await
            .unwrap();

        let log = launcher.log();
        assert_eq!(log.console, [console::SHUTDOWN_COMMAND], "tried first");
        assert_eq!(log.stops_requested, 1, "then escalated");
        assert_eq!(log.kills, 0);
        assert!(outcome.stopped_by_request);
    }

    /// Only a second request kills. Returning after the first would hand the shell back
    /// while the world was still being written.
    #[tokio::test(start_paused = true)]
    async fn a_server_that_ignores_everything_is_killed_only_when_asked_twice() {
        let dir = tempfile::tempdir().unwrap();
        let instance = instance(dir.path());
        let stop = StopHandle::new();
        let launcher = FakeLauncher::new(dir.path(), &[137])
            .stopping(Stops::NotAtAll)
            .insisting_with(&stop);

        let outcome = supervise_stopping(&instance, &launcher, stop.clone())
            .await
            .unwrap();

        let log = launcher.log();
        assert_eq!(log.stops_requested, 1);
        assert_eq!(log.kills, 1);
        assert!(outcome.stopped_by_request);
    }

    /// An update landing exactly as the operator hits Ctrl-C must not quietly bring the
    /// server back up.
    #[tokio::test]
    async fn a_requested_stop_beats_a_restart_request() {
        let dir = tempfile::tempdir().unwrap();
        let instance = instance(dir.path());
        let launcher = FakeLauncher::new(dir.path(), &[RESTART_EXIT_CODE])
            .stopping(Stops::OnTheConsoleCommand);

        let outcome = supervise_stopping(&instance, &launcher, StopHandle::new())
            .await
            .unwrap();

        assert_eq!(outcome.restarts, 0);
        assert_eq!(launcher.log().commands.len(), 1, "it must not start again");
        assert!(outcome.stopped_by_request);
    }
}
