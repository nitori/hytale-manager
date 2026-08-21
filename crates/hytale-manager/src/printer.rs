//! Terminal output.
//!
//! Everything humans read goes to stderr; anything a script would consume — a resolved
//! path, a version — goes to stdout, so `hy java find --executable` can be substituted
//! into a command line.
//!
//! Output goes through [`anstream`], which strips ANSI codes when the stream is not a
//! terminal and honours the global choice set from `--color`. Note that `owo_colors`'
//! `set_override` is *not* sufficient on its own: it only governs `if_supports_color`,
//! while `.bold()` and friends emit codes unconditionally.

use std::fmt::Display;

use owo_colors::OwoColorize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verbosity {
    Quiet,
    Normal,
    Verbose,
}

#[derive(Debug, Clone, Copy)]
pub struct Printer {
    verbosity: Verbosity,
}

impl Printer {
    pub fn new(quiet: bool, verbose: u8) -> Self {
        let verbosity = if quiet {
            Verbosity::Quiet
        } else if verbose > 0 {
            Verbosity::Verbose
        } else {
            Verbosity::Normal
        };
        Self { verbosity }
    }

    pub fn is_quiet(self) -> bool {
        self.verbosity == Verbosity::Quiet
    }

    /// A significant action, e.g. installing a runtime.
    pub fn event(self, message: impl Display) {
        if self.verbosity > Verbosity::Quiet {
            anstream::eprintln!("{}", message);
        }
    }

    /// Supporting detail, indented under the preceding event.
    pub fn detail(self, message: impl Display) {
        if self.verbosity > Verbosity::Quiet {
            anstream::eprintln!("  {}", message.dimmed());
        }
    }

    pub fn warn(self, message: impl Display) {
        if self.verbosity > Verbosity::Quiet {
            anstream::eprintln!("{} {}", "warning:".yellow().bold(), message);
        }
    }

    /// Machine-consumable output.
    pub fn stdout(self, message: impl Display) {
        anstream::println!("{}", message);
    }
}

/// Format a byte count for humans.
pub fn bytes(count: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = count as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{count} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
