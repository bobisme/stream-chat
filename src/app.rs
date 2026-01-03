use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::message::ChatMessage;

pub struct App {
    pub stream_title: String,
    #[allow(dead_code)]
    pub live_chat_id: String,
    pub messages: Vec<ChatMessage>,
    pub input: String,
    pub cursor_position: usize,
    pub scroll_offset: usize,
    pub should_quit: bool,
    pub is_sending: bool,
    pub error_message: Option<String>,
}

pub enum AppAction {
    SendMessage(String),
}

impl App {
    pub fn new(stream_title: String, live_chat_id: String) -> Self {
        Self {
            stream_title,
            live_chat_id,
            messages: Vec::new(),
            input: String::new(),
            cursor_position: 0,
            scroll_offset: 0,
            should_quit: false,
            is_sending: false,
            error_message: None,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<AppAction> {
        match (key.modifiers, key.code) {
            // Quit
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                self.should_quit = true;
                None
            }

            // Scroll down (toward newer messages)
            (KeyModifiers::CONTROL, KeyCode::Char('j')) => {
                self.scroll_down(10);
                None
            }

            // Scroll up (toward older messages)
            (KeyModifiers::CONTROL, KeyCode::Char('k')) => {
                self.scroll_up(10);
                None
            }

            // Send message
            (_, KeyCode::Enter) => {
                if !self.input.is_empty() && !self.is_sending {
                    let message = std::mem::take(&mut self.input);
                    self.cursor_position = 0;
                    Some(AppAction::SendMessage(message))
                } else {
                    None
                }
            }

            // Text input
            (_, KeyCode::Char(c)) => {
                self.input.insert(self.cursor_position, c);
                self.cursor_position += 1;
                None
            }

            (_, KeyCode::Backspace) => {
                if self.cursor_position > 0 {
                    self.cursor_position -= 1;
                    self.input.remove(self.cursor_position);
                }
                None
            }

            (_, KeyCode::Left) => {
                self.cursor_position = self.cursor_position.saturating_sub(1);
                None
            }

            (_, KeyCode::Right) => {
                self.cursor_position = (self.cursor_position + 1).min(self.input.len());
                None
            }

            (_, KeyCode::Home) => {
                self.cursor_position = 0;
                None
            }

            (_, KeyCode::End) => {
                self.cursor_position = self.input.len();
                None
            }

            _ => None,
        }
    }

    pub fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(amount);
    }

    pub fn scroll_down(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    pub fn add_messages(&mut self, new_messages: Vec<ChatMessage>) {
        // If user is at bottom (scroll_offset == 0), stay at bottom
        let was_at_bottom = self.scroll_offset == 0;

        self.messages.extend(new_messages);

        // Keep a reasonable message limit
        if self.messages.len() > 1000 {
            self.messages.drain(0..500);
        }

        // Auto-scroll to bottom if user was already there
        if was_at_bottom {
            self.scroll_offset = 0;
        }
    }
}
