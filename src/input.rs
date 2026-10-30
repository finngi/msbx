//! Bounded single-line editing; no terminal or filesystem access.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Line;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TextInput {
    text: String,
    /// Always a UTF-8 boundary; replacement resets it to the end.
    cursor: usize,
}

impl TextInput {
    pub(crate) fn new(text: &str) -> Self {
        let mut input = Self::default();
        input.set_text(text);
        input
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn set_text(&mut self, text: &str) {
        self.text.clear();
        self.cursor = 0;
        self.insert(text);
    }

    pub(crate) fn insert(&mut self, text: &str) {
        for character in text.chars().filter(|character| !character.is_control()) {
            if self.text.len() + character.len_utf8() > 512 {
                break;
            }
            self.text.insert(self.cursor, character);
            self.cursor += character.len_utf8();
        }
    }

    fn left(&mut self) {
        self.cursor = self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index);
    }

    fn right(&mut self) {
        self.cursor += self.text[self.cursor..]
            .chars()
            .next()
            .map_or(0, char::len_utf8);
    }

    /// Consumes editing keys, including modifiers; global commands are handled by the owner.
    pub(crate) fn update(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('u') {
            self.set_text("");
            return true;
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        match key.code {
            KeyCode::Char(character) => self.insert(&character.to_string()),
            KeyCode::Left => self.left(),
            KeyCode::Right => self.right(),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.len(),
            KeyCode::Backspace => {
                let end = self.cursor;
                self.left();
                self.text.drain(self.cursor..end);
            }
            KeyCode::Delete => {
                let start = self.cursor;
                self.right();
                self.text.drain(start..self.cursor);
                self.cursor = start;
            }
            _ => return false,
        }
        true
    }

    /// Visible suffix ending at the cursor; width is in terminal cells, not bytes.
    pub(crate) fn visible(&self, width: u16) -> (&str, u16) {
        let mut start = self.cursor;
        for (index, _) in self.text[..self.cursor].char_indices().rev() {
            if Line::from(&self.text[index..self.cursor]).width() >= usize::from(width) {
                break;
            }
            start = index;
        }
        let column = u16::try_from(Line::from(&self.text[start..self.cursor]).width()).unwrap_or(0);
        (&self.text[start..], column)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(input: &mut TextInput, code: KeyCode) {
        input.update(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn editing_and_replacing_unicode_keeps_valid_boundaries() {
        let mut input = TextInput::new("a猫é");
        key(&mut input, KeyCode::Left);
        key(&mut input, KeyCode::Backspace);
        assert_eq!(input.text(), "aé");
        key(&mut input, KeyCode::Home);
        key(&mut input, KeyCode::Delete);
        input.set_text("短");
        key(&mut input, KeyCode::End);
        key(&mut input, KeyCode::Char('q'));
        assert_eq!(input.text(), "短q");
        assert_eq!(input.visible(3), ("q", 1));
    }

    #[test]
    fn paste_is_bounded_and_cannot_inject_control_keys() {
        let mut input = TextInput::default();
        input.insert("a\n\r\u{1b}b");
        assert_eq!(input.text(), "ab");
        input.insert(&"猫".repeat(300));
        assert!(input.text().len() <= 512);
        input.update(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(input.text(), "");
        input.update(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::ALT));
        assert_eq!(input.text(), "");
    }
}
