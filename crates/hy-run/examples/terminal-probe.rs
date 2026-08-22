//! Does a TUI actually work in this terminal?
//!
//! The console UI would rest entirely on crossterm being able to take raw mode and read key
//! events. That is uncontroversial on unix and in Windows Terminal, but Git Bash's mintty
//! is not a Windows console — crossterm reads input there through the Console API, which
//! only works when ConPTY is backing the session. Since mintty is precisely where the
//! Ctrl-C fix matters, it is worth proving before building anything on top.
//!
//! Run it in each terminal you care about:
//!
//! ```text
//! cargo run -p hy-run --example terminal-probe
//! ```
//!
//! Type a few keys, press Ctrl-C, then q to quit. Anything that reports a failure, or shows
//! no key events, means the TUI needs a different input strategy there.

use std::io::IsTerminal;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal;

fn main() {
    println!("stdin is a terminal:  {}", std::io::stdin().is_terminal());
    println!("stdout is a terminal: {}", std::io::stdout().is_terminal());
    match terminal::size() {
        Ok((w, h)) => println!("terminal size:        {w}x{h}"),
        Err(err) => println!("terminal size:        FAILED — {err}"),
    }

    if let Err(err) = terminal::enable_raw_mode() {
        println!("\nraw mode:             FAILED — {err}");
        println!("A TUI cannot work here as-is.");
        return;
    }
    println!("raw mode:             ok\r");
    println!("\nPress keys. Ctrl-C should appear as an event, not kill this. `q` quits.\r");

    let mut events = 0;
    let mut saw_ctrl_c = false;

    loop {
        // `poll` is the part that makes this cancellable, unlike a bare blocking read —
        // the TUI's input loop would check a stop flag on each timeout.
        match event::poll(Duration::from_millis(500)) {
            Ok(false) => continue,
            Err(err) => {
                println!("poll FAILED — {err}\r");
                break;
            }
            Ok(true) => {}
        }

        match event::read() {
            Err(err) => {
                println!("read FAILED — {err}\r");
                break;
            }
            Ok(Event::Key(key)) => {
                events += 1;
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                // All modifiers, not just ctrl: a binding that relies on Shift is only as
                // good as the terminal's willingness to report it.
                println!(
                    "  key: {:?}  modifiers: {:?}  kind: {:?}\r",
                    key.code, key.modifiers, key.kind
                );

                if ctrl && key.code == KeyCode::Char('c') {
                    saw_ctrl_c = true;
                    println!("  ^ Ctrl-C arrived as an event — this is what we need.\r");
                }
                if key.code == KeyCode::Char('q') {
                    break;
                }
            }
            Ok(other) => {
                events += 1;
                println!("  {other:?}\r");
            }
        }
    }

    let _ = terminal::disable_raw_mode();
    println!("\nevents seen:   {events}");
    println!("Ctrl-C as event: {saw_ctrl_c}");
    if events == 0 {
        println!("\nNo input got through — a TUI would be unusable in this terminal.");
    } else if saw_ctrl_c {
        println!("\nLooks good: this terminal can host the console UI.");
    } else {
        println!("\nInput works, but Ctrl-C never arrived as an event — worth knowing.");
    }
}
