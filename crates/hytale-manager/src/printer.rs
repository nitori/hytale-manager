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

/// Marks a line as ours. The server's own logging shares this terminal, and its lines carry
/// a `[timestamp LEVEL] [Component]` prefix of their own; without a tag it is not obvious
/// which of the two is speaking.
pub const PREFIX: &str = "[hy]";

/// Dimmed, because it is a marker rather than content.
pub fn tag() -> String {
    PREFIX.dimmed().to_string()
}

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
            anstream::eprintln!("{} {}", tag(), message);
        }
    }

    /// Supporting detail, indented under the preceding event.
    pub fn detail(self, message: impl Display) {
        if self.verbosity > Verbosity::Quiet {
            anstream::eprintln!("{}   {}", tag(), message.dimmed());
        }
    }

    pub fn warn(self, message: impl Display) {
        if self.verbosity > Verbosity::Quiet {
            anstream::eprintln!("{} {} {}", tag(), "warning:".yellow().bold(), message);
        }
    }

    /// Machine-consumable output.
    ///
    /// Never tagged: `hy java find --executable` is meant to be substituted straight into a
    /// command line.
    pub fn stdout(self, message: impl Display) {
        anstream::println!("{}", message);
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Align {
    Left,
    Right,
}

/// Rows printed as columns sized to their contents.
///
/// Cells must be plain text: widths are counted in characters, so an ANSI escape in one
/// would be counted as content and skew the column for every row.
pub struct Table {
    columns: Vec<Align>,
    rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(columns: impl IntoIterator<Item = Align>) -> Self {
        Self {
            columns: columns.into_iter().collect(),
            rows: Vec::new(),
        }
    }

    pub fn row(&mut self, cells: impl IntoIterator<Item = String>) {
        self.rows.push(cells.into_iter().collect());
    }

    pub fn print(&self, printer: Printer) {
        for line in self.render() {
            printer.stdout(line);
        }
    }

    fn render(&self) -> Vec<String> {
        let mut widths: Vec<usize> = Vec::new();
        for row in &self.rows {
            for (column, cell) in row.iter().enumerate() {
                let width = cell.chars().count();
                match widths.get_mut(column) {
                    Some(current) => *current = (*current).max(width),
                    None => widths.push(width),
                }
            }
        }

        let mut lines = Vec::with_capacity(self.rows.len());
        for row in &self.rows {
            let mut line = String::new();
            for (column, cell) in row.iter().enumerate() {
                if column > 0 {
                    line.push_str(GAP);
                }
                // Padding the last cell would only add trailing whitespace.
                if column + 1 == row.len() {
                    line.push_str(cell);
                    continue;
                }
                let pad = widths[column].saturating_sub(cell.chars().count());
                match self.columns.get(column) {
                    Some(Align::Right) => {
                        line.extend(std::iter::repeat_n(' ', pad));
                        line.push_str(cell);
                    }
                    _ => {
                        line.push_str(cell);
                        line.extend(std::iter::repeat_n(' ', pad));
                    }
                }
            }
            lines.push(line.trim_end().to_string());
        }
        lines
    }
}

const GAP: &str = "  ";

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

#[cfg(test)]
mod tests {
    use super::*;

    fn table(columns: impl IntoIterator<Item = Align>, rows: &[&[&str]]) -> Vec<String> {
        let mut table = Table::new(columns);
        for row in rows {
            table.row(row.iter().map(|cell| cell.to_string()));
        }
        table.render()
    }

    #[test]
    fn columns_are_sized_to_their_widest_cell() {
        let lines = table(
            [Align::Left, Align::Left],
            &[&["temurin-25", "/opt/a"], &["temurin-25.0.4.1+1", "/opt/b"]],
        );
        assert_eq!(
            lines,
            ["temurin-25          /opt/a", "temurin-25.0.4.1+1  /opt/b"]
        );
    }

    #[test]
    fn a_right_aligned_column_pads_on_the_left() {
        let lines = table(
            [Align::Left, Align::Right, Align::Left],
            &[&["a", "4.0 GiB", "x"], &["b", "12 B", "y"]],
        );
        assert_eq!(lines, ["a  4.0 GiB  x", "b     12 B  y"]);
    }

    /// The lineage column of `hy backup list` is empty for most rows; padding it would put
    /// invisible whitespace at the end of nearly every line the user copies.
    #[test]
    fn an_empty_trailing_cell_leaves_no_whitespace() {
        let lines = table([Align::Left, Align::Left], &[&["one", ""], &["two", "note"]]);
        assert_eq!(lines, ["one", "two  note"]);
    }

    /// A long value used to break every row after it, back when the widths were guessed.
    #[test]
    fn an_overlong_cell_widens_the_column_rather_than_overflowing_it() {
        let lines = table(
            [Align::Left, Align::Left],
            &[&["short", "a"], &[&"x".repeat(50), "b"]],
        );
        assert_eq!(lines[0], format!("short{}  a", " ".repeat(45)));
        assert_eq!(lines[1], format!("{}  b", "x".repeat(50)));
    }

    #[test]
    fn bytes_scales_to_the_unit() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1024), "1.0 KiB");
        assert_eq!(bytes(1024 * 1024 * 3 / 2), "1.5 MiB");
    }
}
