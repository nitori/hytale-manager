//! Drawing. Layout only — behaviour lives in [`super::state`].

use ansi_to_tui::IntoText;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::Shared;

pub fn render(frame: &mut Frame, shared: &Shared) {
    let [output, input] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).areas(frame.area());

    let mut console = shared.console();
    let height = output.height.saturating_sub(2) as usize;
    let width = output.width.saturating_sub(2) as usize;
    console.set_viewport(height);

    // Taken by value so the borrow of the console ends before the clamp below.
    let visible: Vec<String> = console.visible(height).cloned().collect();
    let lines: Vec<Line> = visible
        .iter()
        .map(|line| {
            // The server colours its own output; keep it rather than flattening to grey.
            line.into_text()
                .ok()
                .and_then(|text| text.lines.into_iter().next())
                .unwrap_or_else(|| Line::from(line.as_str()))
        })
        .collect();

    // Long lines are cropped rather than wrapped, so the offset is bounded by the widest
    // line actually on screen — scrolling into empty space would just look broken.
    let longest = lines.iter().map(Line::width).max().unwrap_or(0);
    console.clamp_horizontal(longest.saturating_sub(width));
    let horizontal = console.horizontal();

    // Where the view is parked, or — when it is not parked at all — how to move it. Saying
    // nothing would leave a paused pane looking like a hung server.
    let parked = !console.is_pinned_to_tail() || horizontal > 0;
    let title = if parked {
        let mut title = String::from(" server —");
        if !console.is_pinned_to_tail() {
            title.push_str(&format!(" back {} lines", console.scroll()));
        }
        if horizontal > 0 {
            title.push_str(&format!(" +{horizontal} cols"));
        }
        format!("{title} (Esc to reset) ")
    } else {
        " server — PgUp/PgDn · Shift+←/→ to scroll ".to_string()
    };

    frame.render_widget(
        Paragraph::new(lines)
            // Offsetting the widget rather than slicing the strings keeps the server's
            // colours attached to the right characters.
            .scroll((0, horizontal as u16))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .title_style(Style::default().add_modifier(Modifier::BOLD)),
            ),
        output,
    );

    let prompt = "> ";
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(prompt, Style::default().fg(Color::Cyan)),
            Span::raw(console.input()),
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" command — ↑/↓ history · Ctrl-C stops the server "),
        ),
        input,
    );

    // Inside the border, past the prompt, at the character the cursor is on.
    frame.set_cursor_position(Position::new(
        input.x + 1 + prompt.len() as u16 + console.cursor() as u16,
        input.y + 1,
    ));
}
