//! The launcher's single-line query buffer, including IME (composition)
//! support via character/UTF-16 index helpers.

use std::ops::Range;

pub(crate) struct SearchInput {
    pub(crate) query: String,
    /// Cursor position measured in characters.
    pub(crate) cursor: usize,
    /// Active IME composition range, measured in characters. `None` when no
    /// input method is composing text.
    pub(crate) marked: Option<Range<usize>>,
}

impl SearchInput {
    pub(crate) fn char_count(&self) -> usize {
        self.query.chars().count()
    }

    pub(crate) fn byte_index(&self) -> usize {
        self.query
            .char_indices()
            .nth(self.cursor)
            .map(|(index, _)| index)
            .unwrap_or(self.query.len())
    }

    pub(crate) fn insert_char(&mut self, ch: char) {
        self.query.insert(self.byte_index(), ch);
        self.cursor += 1;
    }

    /// Insert `text` at the caret (used for clipboard paste). Clipboard text is
    /// normalized to a single line before it reaches this point.
    pub(crate) fn insert_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let byte = self.byte_at_char(self.cursor);
        self.query.insert_str(byte, text);
        self.cursor += text.chars().count();
    }

    pub(crate) fn backspace(&mut self) {
        if self.cursor > 0 {
            let byte = self
                .query
                .char_indices()
                .nth(self.cursor - 1)
                .map(|(index, _)| index)
                .unwrap_or(0);
            self.query.remove(byte);
            self.cursor -= 1;
        }
    }

    pub(crate) fn delete(&mut self) {
        if self.cursor < self.char_count() {
            let byte = self.byte_index();
            self.query.remove(byte);
        }
    }

    pub(crate) fn move_cursor(&mut self, delta: i32) {
        self.cursor = (self.cursor as i32 + delta).clamp(0, self.char_count() as i32) as usize;
    }

    pub(crate) fn set_cursor(&mut self, index: usize) {
        self.cursor = index.min(self.char_count());
    }

    pub(crate) fn utf16_len(&self) -> usize {
        self.query.encode_utf16().count()
    }

    /// Byte offset of the character at `char_index`.
    pub(crate) fn byte_at_char(&self, char_index: usize) -> usize {
        self.query
            .char_indices()
            .nth(char_index)
            .map(|(index, _)| index)
            .unwrap_or(self.query.len())
    }

    /// Replace the given UTF-16 range with `text` (or the active composition,
    /// or insert at the caret when the platform passes no range) and place the
    /// caret after it. Clears any active composition.
    pub(crate) fn replace_utf16(&mut self, range: Option<Range<usize>>, text: &str) {
        match range.and_then(|r| self.utf16_to_chars(r)) {
            Some(range) => {
                let start = self.byte_at_char(range.start);
                let end = self.byte_at_char(range.end);
                self.query.replace_range(start..end, text);
                self.cursor = range.start + text.chars().count();
            }
            None => {
                // The Windows IME passes `None` for the document; replace the
                // active composition when present, else insert at the caret.
                if let Some(marked) = self.marked.clone() {
                    let start = self.byte_at_char(marked.start);
                    let end = self.byte_at_char(marked.end);
                    self.query.replace_range(start..end, text);
                    self.cursor = marked.start + text.chars().count();
                } else {
                    let byte = self.byte_at_char(self.cursor);
                    self.query.insert_str(byte, text);
                    self.cursor += text.chars().count();
                }
            }
        }
        self.marked = None;
    }

    /// Replace text and mark the replacement as an active composition.
    pub(crate) fn replace_and_mark_utf16(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        new_selected: Option<Range<usize>>,
    ) {
        self.replace_utf16(range, text);
        let text_len_chars = text.chars().count();
        let end = self.cursor;
        let start = end.saturating_sub(text_len_chars);
        self.marked = Some(start..end);
        if let Some(sel) = new_selected {
            let utf16_before = self.char_to_utf16(start);
            let utf16_sel = (utf16_before + sel.start)..(utf16_before + sel.end);
            if let Some(chars) = self.utf16_to_chars(utf16_sel) {
                self.cursor = chars.start;
            }
        }
    }

    /// UTF-16 index of a character index.
    pub(crate) fn char_to_utf16(&self, char_index: usize) -> usize {
        self.query
            .chars()
            .take(char_index)
            .map(|c| c.len_utf16())
            .sum()
    }

    /// Character range corresponding to a UTF-16 range.
    pub(crate) fn utf16_to_chars(&self, utf16: Range<usize>) -> Option<Range<usize>> {
        let mut units = 0;
        let mut start = None;
        for (chars, c) in self.query.chars().enumerate() {
            let len = c.len_utf16();
            if start.is_none() && units + len > utf16.start {
                start = Some(chars);
            }
            units += len;
            if units >= utf16.end {
                return start.map(|s| s..chars + 1);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(query: &str) -> SearchInput {
        SearchInput {
            query: query.to_string(),
            cursor: query.chars().count(),
            marked: None,
        }
    }

    #[test]
    fn ime_commit_appends_at_caret() {
        let mut input = input("你好");
        input.replace_utf16(None, "说话");
        assert_eq!(input.query, "你好说话");
        assert_eq!(input.cursor, 4);
        assert!(input.marked.is_none());
    }

    #[test]
    fn ime_composition_replaces_only_the_marked_text() {
        let mut input = input("你好");
        // The IME starts composing "说话" after the existing text.
        input.replace_and_mark_utf16(None, "说话", Some(0..2));
        assert_eq!(input.query, "你好说话");
        assert_eq!(input.marked, Some(2..4));

        // The commit replaces just the marked range, keeping the prefix.
        input.replace_utf16(None, "说话");
        assert_eq!(input.query, "你好说话");
        assert_eq!(input.cursor, 4);
        assert!(input.marked.is_none());
    }

    #[test]
    fn ime_commit_inserts_at_caret_in_the_middle() {
        let mut input = input("你好世界");
        input.cursor = 2;
        input.replace_utf16(None, "呀");
        assert_eq!(input.query, "你好呀世界");
        assert_eq!(input.cursor, 3);
    }

    #[test]
    fn ime_cancel_clears_only_the_composition() {
        let mut input = input("你好");
        input.replace_and_mark_utf16(None, "ni", None);
        assert_eq!(input.query, "你好ni");
        assert_eq!(input.marked, Some(2..4));

        // lparam == 0 cancels the composition with an empty replacement.
        input.replace_utf16(None, "");
        assert_eq!(input.query, "你好");
        assert!(input.marked.is_none());
    }

    #[test]
    fn ascii_replacement_at_caret() {
        let mut input = input("abc");
        input.replace_utf16(None, "d");
        assert_eq!(input.query, "abcd");
        assert_eq!(input.cursor, 4);
    }
}
