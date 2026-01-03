use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::message::ChatMessage;

pub struct App {
    pub stream_title: String,
    #[allow(dead_code)]
    pub live_chat_id: String,
    pub my_username: String,
    pub messages: Vec<ChatMessage>,
    pub input: String,
    pub cursor_position: usize,
    pub scroll_offset: usize,
    pub should_quit: bool,
    pub is_sending: bool,
    pub error_message: Option<String>,
    // Autocomplete state
    pub autocomplete: Option<AutocompleteState>,
}

pub struct AutocompleteState {
    pub matches: Vec<String>,
    pub selected_index: usize,
    pub at_position: usize,      // Position of @ in input
    pub original_prefix: String, // The text after @ that user typed
}

pub enum AppAction {
    SendMessage(String),
}

impl App {
    pub fn new(stream_title: String, live_chat_id: String, my_username: String) -> Self {
        Self {
            stream_title,
            live_chat_id,
            my_username,
            messages: Vec::new(),
            input: String::new(),
            cursor_position: 0,
            scroll_offset: 0,
            should_quit: false,
            is_sending: false,
            error_message: None,
            autocomplete: None,
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

            // Delete previous word (ctrl+w)
            (KeyModifiers::CONTROL, KeyCode::Char('w')) => {
                self.delete_word_backwards();
                None
            }

            // Delete to beginning of line (ctrl+u)
            (KeyModifiers::CONTROL, KeyCode::Char('u')) => {
                self.input.drain(..self.cursor_position);
                self.cursor_position = 0;
                None
            }

            // Go to beginning of line (ctrl+a)
            (KeyModifiers::CONTROL, KeyCode::Char('a')) => {
                self.cursor_position = 0;
                None
            }

            // Go to end of line (ctrl+e)
            (KeyModifiers::CONTROL, KeyCode::Char('e')) => {
                self.cursor_position = self.input.len();
                None
            }

            // Delete character under cursor (ctrl+d) or to end of line if at end
            (KeyModifiers::CONTROL, KeyCode::Char('d')) => {
                if self.cursor_position < self.input.len() {
                    self.input.remove(self.cursor_position);
                }
                None
            }

            // Tab: autocomplete username
            (_, KeyCode::Tab) => {
                self.handle_autocomplete_tab();
                None
            }

            // Escape: cancel autocomplete
            (_, KeyCode::Esc) => {
                if self.autocomplete.is_some() {
                    self.cancel_autocomplete();
                }
                None
            }

            // Send message
            (_, KeyCode::Enter) => {
                self.autocomplete = None; // Clear autocomplete on send
                if !self.input.is_empty() && !self.is_sending {
                    let message = std::mem::take(&mut self.input);
                    self.cursor_position = 0;
                    Some(AppAction::SendMessage(message))
                } else {
                    None
                }
            }

            // Space: confirm autocomplete or insert space
            (_, KeyCode::Char(' ')) => {
                if self.autocomplete.is_some() {
                    self.confirm_autocomplete();
                } else {
                    self.input.insert(self.cursor_position, ' ');
                    self.cursor_position += 1;
                }
                None
            }

            // Text input
            (_, KeyCode::Char(c)) => {
                self.autocomplete = None; // Cancel autocomplete on typing
                self.input.insert(self.cursor_position, c);
                self.cursor_position += 1;
                None
            }

            (_, KeyCode::Backspace) => {
                self.autocomplete = None; // Cancel autocomplete
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

    pub fn delete_word_backwards(&mut self) {
        if self.cursor_position == 0 {
            return;
        }

        // Find the start of the previous word
        let before_cursor = &self.input[..self.cursor_position];

        // Skip trailing whitespace
        let trimmed_len = before_cursor.trim_end().len();

        // Find the last whitespace before the word
        let word_start = before_cursor[..trimmed_len]
            .rfind(char::is_whitespace)
            .map(|pos| pos + 1)
            .unwrap_or(0);

        // Delete from word_start to cursor
        self.input.drain(word_start..self.cursor_position);
        self.cursor_position = word_start;
    }

    fn handle_autocomplete_tab(&mut self) {
        if let Some(ref mut state) = self.autocomplete {
            // Already in autocomplete mode - cycle to next match
            state.selected_index = (state.selected_index + 1) % state.matches.len();
            self.apply_autocomplete_selection();
        } else {
            // Start autocomplete - find @prefix behind cursor
            if let Some((at_pos, prefix)) = self.find_mention_prefix() {
                let matches = self.find_matching_usernames(&prefix);
                if !matches.is_empty() {
                    self.autocomplete = Some(AutocompleteState {
                        matches,
                        selected_index: 0,
                        at_position: at_pos,
                        original_prefix: prefix,
                    });
                    self.apply_autocomplete_selection();
                }
            }
        }
    }

    fn find_mention_prefix(&self) -> Option<(usize, String)> {
        // Look backwards from cursor for @
        let before_cursor = &self.input[..self.cursor_position];

        // Find the last @ that starts a word (preceded by space or start of string)
        let mut search_from = before_cursor.len();
        while let Some(at_pos) = before_cursor[..search_from].rfind('@') {
            // Check if @ is at start or preceded by whitespace
            let valid_start = at_pos == 0
                || before_cursor.chars().nth(at_pos - 1).map_or(false, |c| c.is_whitespace());

            if valid_start {
                let prefix = &before_cursor[at_pos + 1..];
                // Only autocomplete if prefix has no spaces (still typing the username)
                if !prefix.contains(' ') {
                    return Some((at_pos, prefix.to_string()));
                }
            }
            search_from = at_pos;
        }
        None
    }

    fn find_matching_usernames(&self, prefix: &str) -> Vec<String> {
        let prefix_lower = prefix.to_lowercase();

        // Collect unique usernames from chat that match prefix
        let mut seen = std::collections::HashSet::new();
        let mut matches: Vec<String> = self
            .messages
            .iter()
            .filter_map(|msg| {
                let name = msg.author_name.trim_start_matches('@');
                let name_lower = name.to_lowercase();
                if name_lower.starts_with(&prefix_lower) && seen.insert(name_lower) {
                    Some(name.to_string())
                } else {
                    None
                }
            })
            .collect();

        // Sort alphabetically
        matches.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        matches
    }

    fn apply_autocomplete_selection(&mut self) {
        if let Some(ref state) = self.autocomplete {
            let selected = &state.matches[state.selected_index];
            // Replace from @ to cursor with @selected_username
            let new_mention = format!("@{}", selected);
            let after_cursor = self.input[self.cursor_position..].to_string();

            self.input.truncate(state.at_position);
            self.input.push_str(&new_mention);
            self.cursor_position = self.input.len();
            self.input.push_str(&after_cursor);
        }
    }

    fn confirm_autocomplete(&mut self) {
        // Add a space after the username and clear autocomplete state
        self.input.insert(self.cursor_position, ' ');
        self.cursor_position += 1;
        self.autocomplete = None;
    }

    fn cancel_autocomplete(&mut self) {
        if let Some(state) = self.autocomplete.take() {
            // Restore original text
            let after = self.input[self.cursor_position..].to_string();
            self.input.truncate(state.at_position);
            self.input.push('@');
            self.input.push_str(&state.original_prefix);
            self.cursor_position = self.input.len();
            self.input.push_str(&after);
        }
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
