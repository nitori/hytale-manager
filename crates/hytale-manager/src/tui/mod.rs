//! The console UI: server output above, an input line below.
//!
//! Only used when stdout is a terminal. Under systemd or a redirect, `hy run` keeps the
//! plain passthrough it has always had — a TUI there would fill journald with escape
//! sequences.
//!
//! Because we hold the server's stdin either way (see `hy-run`'s `console`), the server's
//! own jline drops to a dumb terminal. That costs nothing here: the prompt, editing, and
//! history are ours, and better placed for it.

mod state;
mod ui;

use std::io::IsTerminal;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use hy_run::{Output, OutputSink, StopHandle};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub use state::Scrollback;

/// How often the UI redraws while idle. Also how quickly it notices a stop was requested.
const TICK: Duration = Duration::from_millis(100);

/// Columns per Shift-arrow. Small enough to land on something, large enough to cross a
/// timestamp prefix in a few presses.
const COLUMN_STEP: usize = 8;

/// Whether a console UI can be drawn at all.
pub fn is_available() -> bool {
    std::io::stdout().is_terminal() && std::io::stdin().is_terminal() && !is_known_bad()
}

/// mintty corrupts the alternate screen when the window is resized — ConPTY reflows and
/// replays content underneath us, which is not something we can correct from this side.
/// Windows Terminal is unaffected, including when it is hosting Git Bash.
///
/// Falling back to plain output costs only the panes: `hy` still owns the server's stdin
/// there, so Ctrl-C still stops it cleanly. `--tui` forces the UI anyway.
fn is_known_bad() -> bool {
    if !cfg!(windows) {
        return false;
    }
    // Windows Terminal advertises itself, and a Git Bash running inside it is fine.
    if std::env::var_os("WT_SESSION").is_some() {
        return false;
    }
    let mintty = std::env::var("TERM_PROGRAM").is_ok_and(|term| term == "mintty");
    mintty || std::env::var_os("MSYSTEM").is_some()
}

/// Shared between the render loop and the supervisor's output tasks.
#[derive(Clone, Default)]
pub struct Shared {
    scrollback: Arc<Mutex<Scrollback>>,
}

impl Shared {
    pub fn scrollback(&self) -> std::sync::MutexGuard<'_, Scrollback> {
        // A poisoned lock would mean a panic mid-render; showing stale output beats
        // taking the server down with it.
        self.scrollback.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A line from `hy` itself rather than the server, so it can be told apart.
    pub fn note(&self, message: impl Into<String>) {
        self.scrollback()
            .push(format!("{} {}", crate::printer::PREFIX, message.into()));
    }

    pub fn as_output(&self) -> Output {
        Output::To(Arc::new(self.clone()))
    }
}

impl OutputSink for Shared {
    fn line(&self, line: String) {
        self.scrollback().push(line);
    }
}

/// Run the UI until `finished` resolves, driving input into `console`.
///
/// Returns whatever the supervisor produced. The terminal is restored on every path,
/// including a panic in the render loop.
pub async fn run<F>(
    shared: Shared,
    console: hy_run::Console,
    stop: StopHandle,
    supervised: F,
) -> Result<F::Output>
where
    F: std::future::Future,
{
    let mut terminal = Screen::enter()?;
    let outcome = drive(&mut terminal, shared, console, stop, supervised).await;
    terminal.leave();
    outcome
}

async fn drive<F>(
    screen: &mut Screen,
    shared: Shared,
    console: hy_run::Console,
    stop: StopHandle,
    supervised: F,
) -> Result<F::Output>
where
    F: std::future::Future,
{
    let mut supervised = std::pin::pin!(supervised);
    let mut stops_requested = 0u8;

    loop {
        // Draw first so the very first frame appears before anything is typed.
        screen.draw(&shared)?;

        tokio::select! {
            outcome = &mut supervised => return Ok(outcome),
            keys = next_keys() => {
                for key in keys? {
                    match key {
                        Action::Send(line) => {
                            if !console.send(&line).await {
                                shared.note("the server is not accepting commands");
                            }
                        }
                        Action::Stop => {
                            stops_requested += 1;
                            if stops_requested == 1 {
                                shared.note("stopping — letting the server save first");
                                shared.note("press Ctrl-C again to force");
                            }
                            stop.stop();
                        }
                    }
                }
            }
        }
    }
}

enum Action {
    Send(String),
    Stop,
}

/// Poll for input on a blocking thread, so the runtime is never parked on a key press.
///
/// `poll` with a timeout is what makes this cancellable, unlike a bare read — the future
/// simply resolves with nothing to do and the loop redraws.
async fn next_keys() -> Result<Vec<Action>> {
    tokio::task::spawn_blocking(read_pending).await?
}

fn read_pending() -> Result<Vec<Action>> {
    let mut actions = Vec::new();
    if !event::poll(TICK)? {
        return Ok(actions);
    }
    // Drain everything buffered, so a paste does not redraw once per character.
    while event::poll(Duration::ZERO)? {
        if let Some(action) = classify(event::read()?) {
            actions.push(action);
        }
    }
    Ok(actions)
}

/// Turn a terminal event into an intent, mutating the shared state for pure editing.
fn classify(event: Event) -> Option<Action> {
    let shared = SHARED.get()?;
    let Event::Key(key) = event else {
        return None;
    };
    // Windows reports press *and* release; acting on both types every character twice.
    if key.kind != KeyEventKind::Press {
        return None;
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    // Plain arrows stay with the input cursor — editing a mistyped command is far more
    // common than reading past the right edge. Shift is free; there is no selection.
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let mut scrollback = shared.scrollback();

    match key.code {
        KeyCode::Char('c') if ctrl => return Some(Action::Stop),
        KeyCode::Char(c) => scrollback.insert(c),
        KeyCode::Backspace => scrollback.backspace(),
        KeyCode::Delete => scrollback.delete(),
        KeyCode::Left if shift => scrollback.scroll_left(COLUMN_STEP),
        KeyCode::Right if shift => scrollback.scroll_right(COLUMN_STEP),
        KeyCode::Left => scrollback.move_left(),
        KeyCode::Right => scrollback.move_right(),
        KeyCode::Home => scrollback.move_home(),
        KeyCode::End => scrollback.move_end(),
        KeyCode::Up => scrollback.recall_previous(),
        KeyCode::Down => scrollback.recall_next(),
        KeyCode::PageUp => scrollback.page_up(),
        KeyCode::PageDown => scrollback.page_down(),
        KeyCode::Esc => scrollback.scroll_to_tail(),
        KeyCode::Enter => {
            let line = scrollback.submit();
            drop(scrollback);
            return line.map(Action::Send);
        }
        _ => {}
    }
    None
}

/// The input classifier runs on a blocking thread with no borrow of the caller, so the
/// state it edits is reached through here.
static SHARED: std::sync::OnceLock<Shared> = std::sync::OnceLock::new();

pub fn install(shared: &Shared) {
    let _ = SHARED.set(shared.clone());
}

/// Owns raw mode and the alternate screen, and gives them back on drop.
struct Screen {
    terminal: Terminal<CrosstermBackend<std::io::Stdout>>,
}

impl Screen {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut out = std::io::stdout();
        crossterm::execute!(out, EnterAlternateScreen)?;
        Ok(Self {
            terminal: Terminal::new(CrosstermBackend::new(out))?,
        })
    }

    fn draw(&mut self, shared: &Shared) -> Result<()> {
        self.terminal.draw(|frame| ui::render(frame, shared))?;
        Ok(())
    }

    fn leave(&mut self) {
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        // A panic in the render loop must not leave the terminal in raw mode.
        self.leave();
    }
}
