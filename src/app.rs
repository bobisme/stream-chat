use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::message::ChatMessage;

pub const MAX_INPUT_CHARS: usize = 200;

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
    pub const fn new(stream_title: String, live_chat_id: String, my_username: String) -> Self {
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
        self.clamp_cursor_to_boundary();

        if key.modifiers == KeyModifiers::CONTROL && self.handle_ctrl_key(key.code) {
            return None;
        }

        if key.code == KeyCode::Enter
            && (key.modifiers.contains(KeyModifiers::SHIFT)
                || key.modifiers.contains(KeyModifiers::CONTROL))
        {
            self.insert_text_at_cursor("\n");
            return None;
        }

        match key.code {
            // Tab: autocomplete username
            KeyCode::Tab => {
                self.handle_autocomplete_tab();
                None
            }

            // Escape: cancel autocomplete
            KeyCode::Esc => {
                if self.autocomplete.is_some() {
                    self.cancel_autocomplete();
                }
                None
            }

            // Send message
            KeyCode::Enter => {
                self.autocomplete = None; // Clear autocomplete on send
                if !self.input.trim().is_empty() && !self.is_sending {
                    let message = std::mem::take(&mut self.input);
                    self.cursor_position = 0;
                    Some(AppAction::SendMessage(message))
                } else {
                    None
                }
            }

            // Space: confirm autocomplete or insert space
            KeyCode::Char(' ') => {
                if self.autocomplete.is_some() {
                    self.confirm_autocomplete();
                } else {
                    self.insert_text_at_cursor(" ");
                }
                None
            }

            // Text input
            KeyCode::Char(c) => {
                self.autocomplete = None; // Cancel autocomplete on typing
                let mut buf = [0u8; 4];
                self.insert_text_at_cursor(c.encode_utf8(&mut buf));
                None
            }

            KeyCode::Backspace => {
                self.autocomplete = None; // Cancel autocomplete
                if self.cursor_position > 0 {
                    let prev = self.prev_char_boundary(self.cursor_position);
                    self.input.drain(prev..self.cursor_position);
                    self.cursor_position = prev;
                }
                None
            }

            KeyCode::Left => {
                self.cursor_position = self.prev_char_boundary(self.cursor_position);
                None
            }

            KeyCode::Right => {
                self.cursor_position = self.next_char_boundary(self.cursor_position);
                None
            }

            KeyCode::Home => {
                self.cursor_position = 0;
                None
            }

            KeyCode::End => {
                self.cursor_position = self.input.len();
                None
            }

            _ => None,
        }
    }

    fn handle_ctrl_key(&mut self, code: KeyCode) -> bool {
        match code {
            // Quit
            KeyCode::Char('c') => {
                self.should_quit = true;
                true
            }

            // Scroll down (toward newer messages)
            KeyCode::Char('j') => {
                self.scroll_down(10);
                true
            }

            // Scroll up (toward older messages)
            KeyCode::Char('k') => {
                self.scroll_up(10);
                true
            }

            // Delete previous word (ctrl+w)
            KeyCode::Char('w') => {
                self.delete_word_backwards();
                true
            }

            // Delete to beginning of line (ctrl+u)
            KeyCode::Char('u') => {
                self.input.drain(..self.cursor_position);
                self.cursor_position = 0;
                true
            }

            // Go to beginning of line (ctrl+a)
            KeyCode::Char('a') => {
                self.cursor_position = 0;
                true
            }

            // Go to end of line (ctrl+e)
            KeyCode::Char('e') => {
                self.cursor_position = self.input.len();
                true
            }

            // Delete character under cursor (ctrl+d) or to end of line if at end
            KeyCode::Char('d') => {
                if self.cursor_position < self.input.len() {
                    let next = self.next_char_boundary(self.cursor_position);
                    self.input.drain(self.cursor_position..next);
                }
                true
            }

            _ => false,
        }
    }

    pub const fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(amount);
    }

    pub const fn scroll_down(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    pub fn delete_word_backwards(&mut self) {
        self.clamp_cursor_to_boundary();

        if self.cursor_position == 0 {
            return;
        }

        // Find the start of the previous word
        let before_cursor = &self.input[..self.cursor_position];

        // Skip trailing whitespace
        let trimmed_len = before_cursor.trim_end().len();

        // Find the last whitespace before the word
        let trimmed = &before_cursor[..trimmed_len];
        let word_start = trimmed.rfind(char::is_whitespace).map_or(0, |pos| {
            let ch_len = trimmed[pos..].chars().next().map_or(1, char::len_utf8);
            pos + ch_len
        });

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
        let cursor = self.cursor_position.min(self.input.len());
        let mut cursor = cursor;
        while cursor > 0 && !self.input.is_char_boundary(cursor) {
            cursor -= 1;
        }

        // Look backwards from cursor for @
        let before_cursor = &self.input[..cursor];

        // Find the last @ that starts a word (preceded by space or start of string)
        let mut search_from = before_cursor.len();
        while let Some(at_pos) = before_cursor[..search_from].rfind('@') {
            // Check if @ is at start or preceded by whitespace
            let valid_start = at_pos == 0
                || before_cursor[..at_pos]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace);

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
        matches.sort_by_key(|a| a.to_lowercase());
        matches
    }

    fn apply_autocomplete_selection(&mut self) {
        self.clamp_cursor_to_boundary();

        if let Some(ref state) = self.autocomplete {
            let selected = &state.matches[state.selected_index];
            // Replace from @ to cursor with @selected_username
            let new_mention = format!("@{selected}");
            let after_cursor = self.input[self.cursor_position..].to_string();

            self.input.truncate(state.at_position);
            self.input.push_str(&new_mention);
            self.cursor_position = self.input.len();
            self.input.push_str(&after_cursor);
            self.trim_to_max_input_chars();
        }
    }

    fn confirm_autocomplete(&mut self) {
        // Add a space after the username and clear autocomplete state
        self.insert_text_at_cursor(" ");
        self.autocomplete = None;
    }

    fn cancel_autocomplete(&mut self) {
        self.clamp_cursor_to_boundary();

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

    pub fn input_char_count(&self) -> usize {
        self.input.chars().count()
    }

    fn remaining_input_chars(&self) -> usize {
        MAX_INPUT_CHARS.saturating_sub(self.input_char_count())
    }

    fn insert_text_at_cursor(&mut self, text: &str) {
        self.clamp_cursor_to_boundary();

        let allowed = self.remaining_input_chars();
        if allowed == 0 {
            return;
        }

        let clipped: String = text.chars().take(allowed).collect();
        if clipped.is_empty() {
            return;
        }

        self.input.insert_str(self.cursor_position, &clipped);
        self.cursor_position += clipped.len();
    }

    fn clamp_cursor_to_boundary(&mut self) {
        self.cursor_position = self.cursor_position.min(self.input.len());
        while self.cursor_position > 0 && !self.input.is_char_boundary(self.cursor_position) {
            self.cursor_position -= 1;
        }
    }

    fn prev_char_boundary(&self, cursor: usize) -> usize {
        let mut idx = cursor.min(self.input.len());
        while idx > 0 && !self.input.is_char_boundary(idx) {
            idx -= 1;
        }

        if idx == 0 {
            return 0;
        }

        let mut prev = idx - 1;
        while prev > 0 && !self.input.is_char_boundary(prev) {
            prev -= 1;
        }
        prev
    }

    fn next_char_boundary(&self, cursor: usize) -> usize {
        let mut idx = cursor.min(self.input.len());
        while idx > 0 && !self.input.is_char_boundary(idx) {
            idx -= 1;
        }

        if idx >= self.input.len() {
            return self.input.len();
        }

        let mut next = idx + 1;
        while next < self.input.len() && !self.input.is_char_boundary(next) {
            next += 1;
        }
        next
    }

    fn trim_to_max_input_chars(&mut self) {
        if self.input_char_count() <= MAX_INPUT_CHARS {
            return;
        }

        self.input = self.input.chars().take(MAX_INPUT_CHARS).collect();
        self.cursor_position = self.cursor_position.min(self.input.len());
        while self.cursor_position > 0 && !self.input.is_char_boundary(self.cursor_position) {
            self.cursor_position -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn handles_unicode_cursor_and_backspace_safely() {
        let mut app = App::new("t".to_string(), "id".to_string(), String::new());
        app.input = "hié".to_string();
        app.cursor_position = app.input.len();

        app.handle_key(key(KeyCode::Left));
        app.handle_key(key(KeyCode::Backspace));

        assert_eq!(app.input, "hé");
        assert_eq!(app.cursor_position, 1);

        app.cursor_position = 2; // intentionally mid-byte within 'é'
        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.cursor_position, app.input.len());
    }

    #[test]
    fn enforces_max_input_chars_on_typing() {
        let mut app = App::new("t".to_string(), "id".to_string(), String::new());

        for _ in 0..(MAX_INPUT_CHARS + 20) {
            app.handle_key(key(KeyCode::Char('a')));
        }

        assert_eq!(app.input.chars().count(), MAX_INPUT_CHARS);
    }
}
