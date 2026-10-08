//! A one-line text box with a cursor: the rename box now, and later the
//! command palette and the invite box.
//!
//! The cursor is a char index, so Thai, accents and emoji move as one step.
//! Pasted text arrives whole (bracketed paste) and is cleaned to one line.

use ratatui::text::Line;

/// The text and where the cursor is in it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Input {
    text: String,
    /// Index in chars, `0..=len`.
    cursor: usize,
    /// Longest text accepted, in chars.
    max: usize,
}

/// An edit a key asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    Left,
    Right,
    WordLeft,
    WordRight,
    Home,
    End,
    Backspace,
    Delete,
    /// Ctrl+Backspace / Ctrl+W.
    DeleteWord,
}

impl Input {
    pub fn new(max: usize) -> Self {
        Self {
            max,
            ..Self::default()
        }
    }

    /// A box that starts with `text`, the cursor at its end.
    pub fn with_text(text: &str, max: usize) -> Self {
        let mut input = Self::new(max);
        input.paste(text);
        input
    }

    /// Replaces the text exactly as given (no trimming), cursor at the end.
    /// For completions, whose trailing space matters.
    pub fn set_text(&mut self, text: &str) {
        self.text = text
            .chars()
            .filter(|c| !c.is_control())
            .take(self.max)
            .collect();
        self.cursor = self.len();
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    fn byte(&self, char_index: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_index)
            .map_or(self.text.len(), |(i, _)| i)
    }

    /// Types one character at the cursor. Control characters are ignored.
    pub fn insert(&mut self, c: char) {
        if c.is_control() || self.len() >= self.max {
            return;
        }
        let at = self.byte(self.cursor);
        self.text.insert(at, c);
        self.cursor += 1;
    }

    /// Inserts pasted text. Line breaks and tabs become spaces, other control
    /// characters are dropped, and surrounding spaces are trimmed — an
    /// invite copied from a chat often comes with a newline.
    pub fn paste(&mut self, text: &str) {
        let one_line: String = text
            .chars()
            .map(|c| {
                if matches!(c, '\n' | '\r' | '\t') {
                    ' '
                } else {
                    c
                }
            })
            .filter(|c| !c.is_control())
            .collect();
        for c in one_line.trim().chars() {
            self.insert(c);
        }
    }

    pub fn edit(&mut self, edit: Edit) {
        let len = self.len();
        match edit {
            Edit::Left => self.cursor = self.cursor.saturating_sub(1),
            Edit::Right => self.cursor = (self.cursor + 1).min(len),
            Edit::Home => self.cursor = 0,
            Edit::End => self.cursor = len,
            Edit::WordLeft => self.cursor = self.word_left(),
            Edit::WordRight => self.cursor = self.word_right(),
            Edit::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    let at = self.byte(self.cursor);
                    self.text.remove(at);
                }
            }
            Edit::Delete => {
                if self.cursor < len {
                    let at = self.byte(self.cursor);
                    self.text.remove(at);
                }
            }
            Edit::DeleteWord => {
                let from = self.word_left();
                let (a, b) = (self.byte(from), self.byte(self.cursor));
                self.text.replace_range(a..b, "");
                self.cursor = from;
            }
        }
    }

    fn chars(&self) -> Vec<char> {
        self.text.chars().collect()
    }

    fn word_left(&self) -> usize {
        let chars = self.chars();
        let mut i = self.cursor;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    fn word_right(&self) -> usize {
        let chars = self.chars();
        let mut i = self.cursor;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        i
    }

    /// Moves the cursor to the character under screen column `column`
    /// (relative to where the visible text starts, after `scroll`).
    pub fn click(&mut self, column: u16, width: u16) {
        let scroll = self.scroll(width);
        let target = scroll as usize + column as usize;
        let mut at = 0;
        let mut cells = 0;
        for c in self.text.chars() {
            let w = Line::from(c.to_string()).width();
            if cells + w > target {
                break;
            }
            cells += w;
            at += 1;
        }
        self.cursor = at;
    }

    /// Screen cells before the cursor.
    fn cells_before_cursor(&self) -> usize {
        let before: String = self.text.chars().take(self.cursor).collect();
        Line::from(before).width()
    }

    /// How many cells to scroll so the cursor stays inside `width`.
    pub fn scroll(&self, width: u16) -> u16 {
        let width = width.max(1) as usize;
        let before = self.cells_before_cursor();
        before.saturating_sub(width - 1) as u16
    }

    /// The cursor's column inside a box `width` cells wide.
    pub fn cursor_column(&self, width: u16) -> u16 {
        (self.cells_before_cursor() as u16).saturating_sub(self.scroll(width))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_moving_and_deleting() {
        let mut input = Input::new(64);
        for c in "alce".chars() {
            input.insert(c);
        }
        input.edit(Edit::Left);
        input.edit(Edit::Left);
        input.insert('i');
        assert_eq!(input.text(), "alice");
        assert_eq!(input.cursor(), 3);
        input.edit(Edit::End);
        input.edit(Edit::Backspace);
        input.edit(Edit::Home);
        input.edit(Edit::Delete);
        assert_eq!(input.text(), "lic");
        // Nothing happens past either end.
        input.edit(Edit::Left);
        input.edit(Edit::Backspace);
        input.edit(Edit::End);
        input.edit(Edit::Delete);
        assert_eq!(input.text(), "lic");
    }

    #[test]
    fn words_jump_and_delete_whole() {
        let mut input = Input::with_text("send alice  report.pdf", 64);
        input.edit(Edit::WordLeft);
        assert_eq!(input.cursor(), 12);
        input.edit(Edit::WordLeft);
        assert_eq!(input.cursor(), 5);
        input.edit(Edit::WordRight);
        assert_eq!(input.cursor(), 12);
        input.edit(Edit::End);
        input.edit(Edit::DeleteWord);
        assert_eq!(input.text(), "send alice  ");
    }

    #[test]
    fn thai_and_emoji_move_as_one_character_each() {
        let mut input = Input::with_text("ไฟล์👍", 64);
        assert_eq!(input.cursor(), 5);
        input.edit(Edit::Backspace);
        assert_eq!(input.text(), "ไฟล์");
        input.edit(Edit::Left);
        input.insert('x');
        assert_eq!(input.text(), "ไฟลx์");
    }

    #[test]
    fn a_paste_is_one_clean_line() {
        let mut input = Input::new(64);
        input.paste("  beam1abc\r\n");
        assert_eq!(input.text(), "beam1abc");
        input.paste("\x1b[31mred\x07");
        assert_eq!(input.text(), "beam1abc[31mred", "escape and bell are gone");
    }

    #[test]
    fn the_limit_holds_for_typing_and_pasting() {
        let mut input = Input::new(3);
        input.paste("abcdef");
        input.insert('g');
        assert_eq!(input.text(), "abc");
    }

    #[test]
    fn the_cursor_stays_visible_in_a_narrow_box() {
        let input = Input::with_text("0123456789", 64);
        assert_eq!(input.scroll(4), 7);
        assert_eq!(input.cursor_column(4), 3);
        let mut input = input;
        input.edit(Edit::Home);
        assert_eq!(input.scroll(4), 0);
        assert_eq!(input.cursor_column(4), 0);
    }

    #[test]
    fn a_click_puts_the_cursor_under_the_mouse() {
        let mut input = Input::with_text("alice", 64);
        input.click(2, 20);
        assert_eq!(input.cursor(), 2);
        input.click(40, 20);
        assert_eq!(input.cursor(), 5, "past the end is the end");
    }
}
