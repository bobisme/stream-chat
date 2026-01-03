use ratatui::{
    layout::{Constraint, Layout, Position},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::app::App;
use crate::message::SuperChatInfo;

pub fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Min(5),    // Chat messages area
        Constraint::Length(3), // Input box
        Constraint::Length(1), // Status/help line
    ])
    .split(frame.area());

    render_chat_area(frame, app, chunks[0]);
    render_input_box(frame, app, chunks[1]);
    render_status_line(frame, app, chunks[2]);
}

fn render_chat_area(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(app.stream_title.as_str());

    let inner_width = area.width.saturating_sub(2) as usize; // Account for borders
    let inner_height = area.height.saturating_sub(2) as usize;

    // Build super chat rankings
    let mut super_chats: Vec<(usize, &SuperChatInfo)> = app
        .messages
        .iter()
        .enumerate()
        .filter_map(|(i, m)| m.super_chat.as_ref().map(|sc| (i, sc)))
        .collect();
    super_chats.sort_by(|a, b| b.1.tier.cmp(&a.1.tier));

    // Build chat lines with proper styling
    let lines: Vec<Line> = app
        .messages
        .iter()
        .enumerate()
        .map(|(idx, msg)| {
            let mut spans = Vec::new();

            // Add rank label for super chats
            if msg.super_chat.is_some() {
                if let Some(rank) = super_chats.iter().position(|(i, _)| *i == idx) {
                    spans.push(Span::styled(
                        format!("#{} ", rank + 1),
                        Style::default().bg(Color::Magenta).fg(Color::Black),
                    ));
                }
            }

            // Author name with role-based color (bold)
            spans.push(Span::styled(
                &msg.author_name,
                Style::default()
                    .fg(msg.author_type.color())
                    .add_modifier(Modifier::BOLD),
            ));

            spans.push(Span::raw(": "));
            spans.push(Span::raw(&msg.message));

            Line::from(spans)
        })
        .collect();

    // Calculate wrapped line count for each message
    let wrapped_heights: Vec<usize> = lines
        .iter()
        .map(|line| {
            let line_width: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            if inner_width == 0 {
                1
            } else {
                (line_width.max(1) + inner_width - 1) / inner_width // Ceiling division
            }
        })
        .collect();

    let total_wrapped_lines: usize = wrapped_heights.iter().sum();

    // Calculate scroll position
    // scroll_offset of 0 means "at the bottom" (newest messages visible)
    // Higher scroll_offset means showing older messages
    let scroll = if total_wrapped_lines > inner_height {
        let max_scroll = total_wrapped_lines.saturating_sub(inner_height);
        let effective_scroll = app.scroll_offset.min(max_scroll);
        max_scroll.saturating_sub(effective_scroll)
    } else {
        0
    };

    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll as u16, 0));

    frame.render_widget(para, area);
}

fn render_input_box(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Chat Here");

    let input = Paragraph::new(app.input.as_str()).block(block);

    frame.render_widget(input, area);

    // Position cursor inside the input box
    frame.set_cursor_position(Position::new(
        area.x + app.cursor_position as u16 + 1,
        area.y + 1,
    ));
}

fn render_status_line(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let status = if let Some(ref err) = app.error_message {
        Line::from(Span::styled(err.as_str(), Style::default().fg(Color::Red)))
    } else if app.is_sending {
        Line::from(Span::styled("Sending...", Style::default().fg(Color::Yellow)))
    } else {
        Line::from(Span::styled(
            "Quit: ctrl+c | Scroll: ctrl+j/k",
            Style::default().fg(Color::Blue),
        ))
    };

    frame.render_widget(Paragraph::new(status), area);
}
