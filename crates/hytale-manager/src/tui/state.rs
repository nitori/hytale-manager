//! What the console UI is showing, independent of how it is drawn.
//!
//! Kept apart from rendering so the behaviour that matters — scrollback trimming, history,
//! whether the view is pinned to the tail — is testable without a terminal.

use std::collections::VecDeque;

/// Lines kept above the fold. A busy server produces a few hundred during boot alone, and
/// an unbounded buffer on a long-running process is a slow leak.
pub const SCROLLBACK: usize = 5_000;

/// How many commands to remember for the up/down keys.
const HISTORY: usize = 200;

#[derive(Debug, Default)]
pub struct Scrollback {
    lines: VecDeque<String>,
    /// Lines scrolled up from the bottom. Zero means pinned to the newest output.
    scroll: usize,
    /// Columns scrolled right. Long log lines are cropped rather than wrapped, so this is
    /// how the rest of one is read.
    horizontal: usize,
    /// Rows the output pane last drew, so a page key moves an actual page. Only the
    /// renderer knows it.
    viewport: usize,
    input: String,
    cursor: usize,
    history: VecDeque<String>,
    /// Position while walking back through history; `None` means editing a fresh line.
    recalled: Option<usize>,
}

impl Scrollback {
    pub fn push(&mut self, line: String) {
        if self.lines.len() == SCROLLBACK {
            self.lines.pop_front();
        }
        self.lines.push_back(line);

        // `scroll` counts back from the newest line, so appending moves the text a reader
        // is looking at one further away. Left alone, a paused view slides forward a line
        // per message — which is exactly what pausing was meant to prevent.
        if self.scroll > 0 {
            self.scroll = (self.scroll + 1).min(self.lines.len());
        }
    }

    /// Only the tests need the whole buffer; the UI draws through [`Scrollback::visible`].
    #[cfg(test)]
    pub fn lines(&self) -> &VecDeque<String> {
        &self.lines
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn is_pinned_to_tail(&self) -> bool {
        self.scroll == 0
    }

    /// The window of lines to draw for a viewport `height` tall.
    pub fn visible(&self, height: usize) -> impl Iterator<Item = &String> {
        let end = self.lines.len().saturating_sub(self.scroll);
        let start = end.saturating_sub(height);
        self.lines.range(start..end)
    }

    pub fn set_viewport(&mut self, height: usize) {
        self.viewport = height;
    }

    /// A page, less one line of overlap so nothing is skipped between screens.
    fn page(&self) -> usize {
        self.viewport.saturating_sub(1).max(1)
    }

    pub fn page_up(&mut self) {
        self.scroll_up(self.page(), self.viewport);
    }

    pub fn page_down(&mut self) {
        self.scroll_down(self.page());
    }

    pub fn scroll_up(&mut self, amount: usize, height: usize) {
        let furthest = self.lines.len().saturating_sub(height);
        self.scroll = (self.scroll + amount).min(furthest);
    }

    pub fn scroll_down(&mut self, amount: usize) {
        self.scroll = self.scroll.saturating_sub(amount);
    }

    /// Esc resets the view entirely — both axes — so there is one key that always gets
    /// back to "following the newest output from the left margin".
    pub fn scroll_to_tail(&mut self) {
        self.scroll = 0;
        self.horizontal = 0;
    }

    pub fn horizontal(&self) -> usize {
        self.horizontal
    }

    pub fn scroll_right(&mut self, amount: usize) {
        self.horizontal += amount;
    }

    pub fn scroll_left(&mut self, amount: usize) {
        self.horizontal = self.horizontal.saturating_sub(amount);
    }

    /// Pull the offset back to what the widest visible line justifies.
    ///
    /// Only the renderer knows how wide the content and the pane are, and without this a
    /// run of Shift-Right past the end would need the same number of presses to undo.
    pub fn clamp_horizontal(&mut self, furthest: usize) {
        self.horizontal = self.horizontal.min(furthest);
    }

    pub fn insert(&mut self, c: char) {
        let at = self.byte_offset(self.cursor);
        self.input.insert(at, c);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let at = self.byte_offset(self.cursor - 1);
        self.input.remove(at);
        self.cursor -= 1;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.input.chars().count() {
            return;
        }
        let at = self.byte_offset(self.cursor);
        self.input.remove(at);
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.input.chars().count());
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.input.chars().count();
    }

    /// Take the current line for sending, clearing the box and recording it.
    pub fn submit(&mut self) -> Option<String> {
        let line = std::mem::take(&mut self.input);
        self.cursor = 0;
        self.recalled = None;

        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }
        // Repeating a command should not fill the history with copies of it.
        if self.history.back().map(String::as_str) != Some(trimmed) {
            if self.history.len() == HISTORY {
                self.history.pop_front();
            }
            self.history.push_back(trimmed.to_string());
        }
        Some(trimmed.to_string())
    }

    pub fn recall_previous(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = match self.recalled {
            None => self.history.len() - 1,
            Some(0) => 0,
            Some(index) => index - 1,
        };
        self.recalled = Some(next);
        self.input = self.history[next].clone();
        self.move_end();
    }

    pub fn recall_next(&mut self) {
        let Some(index) = self.recalled else {
            return;
        };
        if index + 1 >= self.history.len() {
            // Past the newest entry is the line the operator was writing: empty.
            self.recalled = None;
            self.input.clear();
            self.cursor = 0;
            return;
        }
        self.recalled = Some(index + 1);
        self.input = self.history[index + 1].clone();
        self.move_end();
    }

    fn byte_offset(&self, chars: usize) -> usize {
        self.input
            .char_indices()
            .nth(chars)
            .map_or(self.input.len(), |(at, _)| at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_lines(count: usize) -> Scrollback {
        let mut scrollback = Scrollback::default();
        for n in 0..count {
            scrollback.push(format!("line {n}"));
        }
        scrollback
    }

    #[test]
    fn scrollback_is_bounded() {
        let scrollback = with_lines(SCROLLBACK + 100);
        assert_eq!(scrollback.lines().len(), SCROLLBACK);
        assert_eq!(
            scrollback.lines().back().unwrap(),
            &format!("line {}", SCROLLBACK + 99)
        );
    }

    #[test]
    fn the_view_shows_the_newest_lines_by_default() {
        let scrollback = with_lines(100);
        let visible: Vec<&String> = scrollback.visible(3).collect();
        assert_eq!(visible, ["line 97", "line 98", "line 99"]);
    }

    #[test]
    fn scrolling_up_moves_back_through_history() {
        let mut scrollback = with_lines(100);
        scrollback.scroll_up(10, 3);
        let visible: Vec<&String> = scrollback.visible(3).collect();
        assert_eq!(visible, ["line 87", "line 88", "line 89"]);
        assert!(!scrollback.is_pinned_to_tail());

        scrollback.scroll_to_tail();
        assert!(scrollback.is_pinned_to_tail());
    }

    #[test]
    fn scrolling_cannot_pass_the_oldest_line() {
        let mut scrollback = with_lines(10);
        scrollback.scroll_up(1000, 4);
        let visible: Vec<&String> = scrollback.visible(4).collect();
        assert_eq!(visible, ["line 0", "line 1", "line 2", "line 3"]);
    }

    /// A paused reader should stay on the same text while new output arrives — both while
    /// the buffer is still filling, and once it has started trimming from the front.
    #[test]
    fn a_scrolled_view_does_not_drift_as_output_arrives() {
        let mut scrollback = with_lines(100);
        scrollback.scroll_up(10, 3);
        let before: Vec<String> = scrollback.visible(3).cloned().collect();

        for n in 0..5 {
            scrollback.push(format!("new {n}"));
        }
        assert_eq!(scrollback.visible(3).cloned().collect::<Vec<_>>(), before);
    }

    #[test]
    fn a_scrolled_view_does_not_drift_once_the_buffer_is_trimming() {
        let mut scrollback = with_lines(SCROLLBACK);
        scrollback.scroll_up(10, 3);
        let before: Vec<String> = scrollback.visible(3).cloned().collect();

        for n in 0..5 {
            scrollback.push(format!("new {n}"));
        }
        assert_eq!(scrollback.visible(3).cloned().collect::<Vec<_>>(), before);
    }

    #[test]
    fn a_page_key_moves_a_page_of_the_pane_actually_drawn() {
        let mut scrollback = with_lines(100);
        scrollback.set_viewport(20);

        scrollback.page_up();
        // One line of overlap, so nothing is skipped between screens.
        assert_eq!(scrollback.scroll(), 19);
        scrollback.page_up();
        assert_eq!(scrollback.scroll(), 38);

        scrollback.page_down();
        assert_eq!(scrollback.scroll(), 19);
    }

    #[test]
    fn paging_stops_at_the_oldest_line() {
        let mut scrollback = with_lines(30);
        scrollback.set_viewport(20);
        for _ in 0..10 {
            scrollback.page_up();
        }
        // The furthest back is the oldest line at the top of a full pane.
        assert_eq!(scrollback.scroll(), 10);
        let visible: Vec<&String> = scrollback.visible(20).collect();
        assert_eq!(visible.first().unwrap().as_str(), "line 0");
    }

    /// Before the first frame there is no pane, and a page key must still do something
    /// sane rather than divide the view by zero.
    #[test]
    fn paging_without_a_viewport_still_moves() {
        let mut scrollback = with_lines(100);
        scrollback.page_up();
        assert_eq!(scrollback.scroll(), 1);
    }

    #[test]
    fn horizontal_scrolling_moves_and_clamps() {
        let mut scrollback = with_lines(10);
        assert_eq!(scrollback.horizontal(), 0);

        scrollback.scroll_right(8);
        scrollback.scroll_right(8);
        assert_eq!(scrollback.horizontal(), 16);

        scrollback.scroll_left(8);
        assert_eq!(scrollback.horizontal(), 8);

        // Never past the left margin, however many times it is pressed.
        scrollback.scroll_left(1000);
        assert_eq!(scrollback.horizontal(), 0);
    }

    /// Scrolling far past the widest line must not need as many presses to undo.
    #[test]
    fn clamping_pulls_the_offset_back_to_the_content() {
        let mut scrollback = with_lines(10);
        for _ in 0..50 {
            scrollback.scroll_right(8);
        }
        scrollback.clamp_horizontal(24);
        assert_eq!(scrollback.horizontal(), 24);

        scrollback.scroll_left(8);
        assert_eq!(
            scrollback.horizontal(),
            16,
            "one press should move one step back"
        );
    }

    #[test]
    fn esc_resets_both_axes() {
        let mut scrollback = with_lines(100);
        scrollback.scroll_up(10, 3);
        scrollback.scroll_right(16);

        scrollback.scroll_to_tail();
        assert!(scrollback.is_pinned_to_tail());
        assert_eq!(scrollback.horizontal(), 0);
    }

    #[test]
    fn following_the_tail_keeps_showing_new_output() {
        let mut scrollback = with_lines(100);
        assert!(scrollback.is_pinned_to_tail());
        scrollback.push("newest".to_string());

        let visible: Vec<&String> = scrollback.visible(3).collect();
        assert_eq!(visible.last().unwrap().as_str(), "newest");
    }

    #[test]
    fn typing_and_editing_respects_character_boundaries() {
        let mut scrollback = Scrollback::default();
        for c in "héllo".chars() {
            scrollback.insert(c);
        }
        assert_eq!(scrollback.input(), "héllo");

        scrollback.move_home();
        scrollback.move_right();
        scrollback.delete();
        // The multi-byte char must be removed whole, not split.
        assert_eq!(scrollback.input(), "hllo");

        scrollback.move_end();
        scrollback.backspace();
        assert_eq!(scrollback.input(), "hll");
    }

    #[test]
    fn submitting_clears_the_box_and_ignores_blank_lines() {
        let mut scrollback = Scrollback::default();
        for c in "  shutdown  ".chars() {
            scrollback.insert(c);
        }
        assert_eq!(scrollback.submit().as_deref(), Some("shutdown"));
        assert_eq!(scrollback.input(), "");
        assert_eq!(scrollback.cursor(), 0);

        for c in "   ".chars() {
            scrollback.insert(c);
        }
        assert_eq!(scrollback.submit(), None);
    }

    #[test]
    fn history_walks_back_and_forward() {
        let mut scrollback = Scrollback::default();
        for command in ["help", "shutdown"] {
            for c in command.chars() {
                scrollback.insert(c);
            }
            scrollback.submit();
        }

        scrollback.recall_previous();
        assert_eq!(scrollback.input(), "shutdown");
        scrollback.recall_previous();
        assert_eq!(scrollback.input(), "help");
        scrollback.recall_next();
        assert_eq!(scrollback.input(), "shutdown");
        // Past the newest is the empty line being composed.
        scrollback.recall_next();
        assert_eq!(scrollback.input(), "");
    }

    #[test]
    fn repeating_a_command_does_not_duplicate_history() {
        let mut scrollback = Scrollback::default();
        for _ in 0..3 {
            for c in "help".chars() {
                scrollback.insert(c);
            }
            scrollback.submit();
        }
        scrollback.recall_previous();
        assert_eq!(scrollback.input(), "help");
        scrollback.recall_previous();
        assert_eq!(scrollback.input(), "help", "there should be only one entry");
    }
}
