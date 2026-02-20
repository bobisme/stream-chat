use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
};

use crate::app::{App, MAX_INPUT_CHARS};
use crate::message::{Badge, BadgeKind};

const INPUT_MIN_LINES: usize = 1;
const INPUT_MAX_LINES: usize = 8;

pub fn render(frame: &mut Frame, app: &App) {
    let input_height = compute_input_height(app, frame.area().width);

    let chunks = Layout::vertical([
        Constraint::Min(5),               // Chat messages area
        Constraint::Length(input_height), // Input box (multiline, dynamic height)
        Constraint::Length(1),            // Status/help line
    ])
    .split(frame.area());

    render_chat_area(frame, app, chunks[0]);
    render_input_box(frame, app, chunks[1]);
    render_status_line(frame, app, chunks[2]);
}

fn render_chat_area(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(app.stream_title.as_str());

    let inner_width = area.width.saturating_sub(2) as usize; // Account for borders
    let inner_height = area.height.saturating_sub(2) as usize;

    // Build chat lines with proper styling
    let lines: Vec<Line> = app
        .messages
        .iter()
        .map(|msg| {
            let mut spans = Vec::new();

            // Author name with role-based color
            spans.push(Span::styled(
                &msg.author_name,
                Style::default().fg(msg.author_type.color()),
            ));

            for badge in &msg.badges {
                if badge.kind == BadgeKind::Moderator {
                    continue;
                }

                spans.push(Span::raw(" "));
                spans.push(Span::styled(format_badge_text(badge), badge_style(badge)));
            }

            spans.push(Span::raw("  "));

            // Highlight mentions of the user
            if app.my_username.is_empty() {
                spans.push(Span::raw(&msg.message));
            } else {
                let mention = format!("@{}", app.my_username);
                let mention_lower = mention.to_lowercase();
                let msg_lower = msg.message.to_lowercase();

                if msg_lower.contains(&mention_lower) {
                    // Split message and highlight mentions
                    let mut remaining = msg.message.as_str();
                    let mut remaining_lower = msg_lower.as_str();

                    while let Some(pos) = remaining_lower.find(&mention_lower) {
                        // Add text before mention
                        if pos > 0 {
                            spans.push(Span::raw(remaining[..pos].to_string()));
                        }
                        // Add highlighted mention
                        spans.push(Span::styled(
                            remaining[pos..pos + mention.len()].to_string(),
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        ));
                        remaining = &remaining[pos + mention.len()..];
                        remaining_lower = &remaining_lower[pos + mention.len()..];
                    }
                    // Add remaining text
                    if !remaining.is_empty() {
                        spans.push(Span::raw(remaining.to_string()));
                    }
                } else {
                    spans.push(Span::raw(&msg.message));
                }
            }

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
                line_width.max(1).div_ceil(inner_width) // Ceiling division
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
        .scroll((usize_to_u16_saturating(scroll), 0));

    frame.render_widget(para, area);
}

fn format_badge_text(badge: &Badge) -> String {
    if is_verified_badge(&badge.text) {
        return "✓".to_string();
    }

    let mut text = badge.text.trim().to_string();
    if text.chars().count() > 20 {
        text = text.chars().take(19).collect::<String>();
        text.push_str("...");
    }
    format!("[{text}]")
}

fn badge_style(badge: &Badge) -> Style {
    if is_verified_badge(&badge.text) {
        return Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::BOLD);
    }

    match badge.kind {
        BadgeKind::Owner => Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        BadgeKind::Moderator => Style::default()
            .fg(Color::White)
            .bg(Color::Blue)
            .add_modifier(Modifier::BOLD),
        BadgeKind::Member => Style::default()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD),
        BadgeKind::Rank => Style::default()
            .fg(Color::Rgb(244, 235, 255))
            .bg(Color::Rgb(66, 25, 118))
            .add_modifier(Modifier::BOLD),
        BadgeKind::Other => Style::default().fg(Color::Cyan),
    }
}

fn is_verified_badge(text: &str) -> bool {
    let lower = text.trim().to_ascii_lowercase();
    lower == "verified" || lower.contains("verified channel") || lower.contains("verified account")
}

fn render_input_box(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title("Chat");

    let inner_width = input_content_width(area.width);
    let inner_height = area.height.saturating_sub(2).max(1);
    let (cursor_col, cursor_row) =
        input_cursor_position(&app.input, app.cursor_position, inner_width);
    let scroll_y = cursor_row.saturating_sub(inner_height.saturating_sub(1));

    let input = Paragraph::new(app.input.as_str())
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll_y, 0));

    frame.render_widget(input, area);

    // Position cursor inside the input box
    frame.set_cursor_position(Position::new(
        area.x + cursor_col + 1,
        area.y + (cursor_row - scroll_y) + 1,
    ));
}

fn render_status_line(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let status = app.autocomplete.as_ref().map_or_else(
        || match app.error_message.as_ref() {
            Some(err) => Line::from(Span::styled(err.as_str(), Style::default().fg(Color::Red))),
            None if app.is_sending => Line::from(Span::styled(
                "Sending...",
                Style::default().fg(Color::Yellow),
            )),
            None => {
                let char_count = app.input_char_count();
                Line::from(Span::styled(
                    format!(
                        "Quit: ctrl+c | Scroll: ctrl+j/k | Newline: shift+enter | {char_count}/{MAX_INPUT_CHARS}"
                    ),
                    Style::default().fg(Color::Blue),
                ))
            }
        },
        |autocomplete| {
            // Show autocomplete options
            let mut spans = vec![Span::styled("@", Style::default().fg(Color::Gray))];

            for (i, name) in autocomplete.matches.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw(" "));
                }

                if i == autocomplete.selected_index {
                    // Highlight selected option
                    spans.push(Span::styled(
                        name.as_str(),
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ));
                } else {
                    spans.push(Span::styled(
                        name.as_str(),
                        Style::default().fg(Color::Cyan),
                    ));
                }

                // Limit display to prevent overflow
                if i >= 5 && autocomplete.matches.len() > 6 {
                    spans.push(Span::styled(
                        format!(" +{}", autocomplete.matches.len() - 6),
                        Style::default().fg(Color::DarkGray),
                    ));
                    break;
                }
            }

            spans.push(Span::styled(
                " | Tab: next, Space: confirm, Esc: cancel",
                Style::default().fg(Color::DarkGray),
            ));

            Line::from(spans)
        },
    );

    frame.render_widget(Paragraph::new(status), area);
}

fn compute_input_height(app: &App, total_width: u16) -> u16 {
    let content_width = input_content_width(total_width);
    let lines =
        visual_input_lines(&app.input, content_width).clamp(INPUT_MIN_LINES, INPUT_MAX_LINES);
    usize_to_u16_saturating(lines).saturating_add(2)
}

fn input_content_width(total_width: u16) -> usize {
    total_width.saturating_sub(2).max(1) as usize
}

fn visual_input_lines(input: &str, width: usize) -> usize {
    if input.is_empty() {
        return 1;
    }

    input
        .split('\n')
        .map(|line| {
            let chars = line.chars().count().max(1);
            chars.div_ceil(width)
        })
        .sum::<usize>()
        .max(1)
}

fn input_cursor_position(input: &str, cursor_position: usize, width: usize) -> (u16, u16) {
    let mut cursor = cursor_position.min(input.len());
    while cursor > 0 && !input.is_char_boundary(cursor) {
        cursor -= 1;
    }

    let before = &input[..cursor];
    let mut row: u16 = 0;
    let mut col: u16 = 0;

    for ch in before.chars() {
        if ch == '\n' {
            row = row.saturating_add(1);
            col = 0;
            continue;
        }

        col = col.saturating_add(1);
        if col as usize >= width {
            row = row.saturating_add(1);
            col = 0;
        }
    }

    (col, row)
}

fn usize_to_u16_saturating(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}
