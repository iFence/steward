//! The launcher's single-line query buffer, including IME (composition)
//! support via character/UTF-16 index helpers.

use std::ops::Range;

pub(crate) struct SearchInput {
    pub(crate) query: String,
    /// Caret (or selection head) position measured in characters.
    pub(crate) cursor: usize,
    /// Active IME composition range, measured in characters. `None` when no
    /// input method is composing text.
    pub(crate) marked: Option<Range<usize>>,
    /// Active text selection (Ctrl+A, mouse drag, platform edits), measured
    /// in characters and normalized so `start <= end`. The caret
    /// (`cursor`) is the selection head. `None` when no text is selected.
    pub(crate) selection: Option<Range<usize>>,
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
        self.delete_selection();
        self.query.insert(self.byte_index(), ch);
        self.cursor += 1;
    }

    /// Insert `text` at the caret (used for clipboard paste). Clipboard text is
    /// normalized to a single line before it reaches this point. Replaces the
    /// active selection when one exists.
    pub(crate) fn insert_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.delete_selection();
        let byte = self.byte_at_char(self.cursor);
        self.query.insert_str(byte, text);
        self.cursor += text.chars().count();
    }

    pub(crate) fn backspace(&mut self) {
        // Backspace over a selection deletes the whole selection.
        if self.delete_selection() {
            return;
        }
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
        // Delete over a selection removes the whole selection.
        if self.delete_selection() {
            return;
        }
        if self.cursor < self.char_count() {
            let byte = self.byte_index();
            self.query.remove(byte);
        }
    }

    pub(crate) fn move_cursor(&mut self, delta: i32) {
        self.cursor = (self.cursor as i32 + delta).clamp(0, self.char_count() as i32) as usize;
        // Navigating collapses the selection to a caret.
        self.selection = None;
    }

    pub(crate) fn set_cursor(&mut self, index: usize) {
        self.cursor = index.min(self.char_count());
        self.selection = None;
    }

    /// Select every character (Ctrl+A). The caret lands after the selection.
    pub(crate) fn select_all(&mut self) {
        let len = self.char_count();
        self.selection = Some(0..len);
        self.cursor = len;
    }

    /// Extend the selection from a fixed `anchor` to `head` (mouse drag). The
    /// caret follows `head`; an empty range collapses back to a caret.
    pub(crate) fn select_anchor_to(&mut self, anchor: usize, head: usize) {
        let anchor = anchor.min(self.char_count());
        let head = head.min(self.char_count());
        let (start, end) = if anchor <= head {
            (anchor, head)
        } else {
            (head, anchor)
        };
        self.cursor = head;
        self.selection = if start == end { None } else { Some(start..end) };
    }

    /// Replace the selection with a normalized character range, placing the
    /// caret at its end. An empty range just collapses the selection.
    pub(crate) fn set_selection(&mut self, range: Range<usize>) {
        let start = range.start.min(range.end).min(self.char_count());
        let end = range.start.max(range.end).min(self.char_count());
        self.selection = if start == end { None } else { Some(start..end) };
        self.cursor = end;
    }

    /// The selected text, or `None` when nothing is selected.
    pub(crate) fn selected_text(&self) -> Option<String> {
        let range = self.selection.clone()?;
        let start = self.byte_at_char(range.start);
        let end = self.byte_at_char(range.end);
        Some(self.query[start..end].to_string())
    }

    /// Delete the selected text and move the caret to where it started.
    /// Returns whether a (non-empty) selection was removed.
    pub(crate) fn delete_selection(&mut self) -> bool {
        if let Some(range) = self.selection.take() {
            let start = self.byte_at_char(range.start);
            let end = self.byte_at_char(range.end);
            if start != end {
                self.query.drain(start..end);
            }
            self.cursor = range.start;
            true
        } else {
            false
        }
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

    /// Character index of a UTF-16 offset, or `None` when the offset lands mid
    /// character or past the end. Used to translate platform caret placements.
    pub(crate) fn utf16_to_char_index(&self, utf16_index: usize) -> Option<usize> {
        let mut units = 0;
        for (chars, c) in self.query.chars().enumerate() {
            if units == utf16_index {
                return Some(chars);
            }
            let len = c.len_utf16();
            if units + len == utf16_index {
                return Some(chars + 1);
            }
            units += len;
        }
        (utf16_index == units).then_some(self.query.chars().count())
    }

    /// Replace the given UTF-16 range with `text` (or the active composition,
    /// or the current selection, or insert at the caret when the platform
    /// passes no range) and place the caret after it. Clears any active
    /// composition and selection.
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
                // active composition when present, else the active selection,
                // else insert at the caret.
                if let Some(marked) = self.marked.clone() {
                    let start = self.byte_at_char(marked.start);
                    let end = self.byte_at_char(marked.end);
                    self.query.replace_range(start..end, text);
                    self.cursor = marked.start + text.chars().count();
                } else {
                    self.delete_selection();
                    let byte = self.byte_at_char(self.cursor);
                    self.query.insert_str(byte, text);
                    self.cursor += text.chars().count();
                }
            }
        }
        self.marked = None;
        self.selection = None;
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
            selection: None,
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

    #[test]
    fn select_all_selects_the_whole_query() {
        let mut input = input("你好世界");
        input.select_all();
        assert_eq!(input.selection, Some(0..4));
        assert_eq!(input.cursor, 4);
        assert_eq!(input.selected_text().as_deref(), Some("你好世界"));
    }

    #[test]
    fn typing_replaces_the_selection() {
        let mut input = input("abcdef");
        input.select_anchor_to(2, 5);
        assert_eq!(input.selection, Some(2..5));
        assert_eq!(input.cursor, 5);
        input.insert_char('X');
        assert_eq!(input.query, "abXf");
        assert_eq!(input.cursor, 3);
        assert!(input.selection.is_none());
    }

    #[test]
    fn mouse_drag_normalizes_an_inverted_range() {
        let mut input = input("abcdef");
        // Dragging from index 5 back to index 2: the range is normalized and
        // the caret (head) follows the drag point.
        input.select_anchor_to(5, 2);
        assert_eq!(input.selection, Some(2..5));
        assert_eq!(input.cursor, 2);
        assert_eq!(input.selected_text().as_deref(), Some("cde"));
    }

    #[test]
    fn drag_collapsing_to_a_caret_clears_the_selection() {
        let mut input = input("abc");
        input.select_anchor_to(0, 2);
        input.select_anchor_to(0, 0);
        assert!(input.selection.is_none());
        assert_eq!(input.cursor, 0);
    }

    #[test]
    fn backspace_deletes_the_selection() {
        let mut input = input("abcdef");
        input.set_selection(1..4);
        input.backspace();
        assert_eq!(input.query, "aef");
        assert_eq!(input.cursor, 1);
        assert!(input.selection.is_none());
    }

    #[test]
    fn delete_deletes_the_selection() {
        let mut input = input("abcdef");
        input.set_selection(1..4);
        input.delete();
        assert_eq!(input.query, "aef");
        assert_eq!(input.cursor, 1);
    }

    #[test]
    fn paste_replaces_the_selection() {
        let mut input = input("abcdef");
        input.set_selection(2..4);
        input.insert_str("XY");
        assert_eq!(input.query, "abXYef");
        assert_eq!(input.cursor, 4);
    }

    #[test]
    fn cut_via_delete_selection_leaves_no_selection() {
        let mut input = input("你好世界");
        input.select_all();
        assert_eq!(input.selected_text().as_deref(), Some("你好世界"));
        input.delete_selection();
        assert!(input.query.is_empty());
        assert_eq!(input.cursor, 0);
        assert!(input.selection.is_none());
    }

    #[test]
    fn navigating_collapses_the_selection() {
        let mut input = input("abc");
        input.select_all();
        input.move_cursor(-1);
        assert!(input.selection.is_none());
        assert_eq!(input.cursor, 2);

        input.select_all();
        input.set_cursor(0);
        assert!(input.selection.is_none());
        assert_eq!(input.cursor, 0);
    }

    #[test]
    fn ime_commit_replaces_the_selection() {
        let mut input = input("你好世界");
        input.select_anchor_to(1, 3);
        input.replace_utf16(None, "呀");
        assert_eq!(input.query, "你呀界");
        assert_eq!(input.cursor, 2);
        assert!(input.selection.is_none());
    }

    #[test]
    fn utf16_to_char_index_maps_offsets() {
        let input = input("你好ab");
        assert_eq!(input.utf16_to_char_index(0), Some(0));
        assert_eq!(input.utf16_to_char_index(1), Some(1));
        assert_eq!(input.utf16_to_char_index(2), Some(2));
        assert_eq!(input.utf16_to_char_index(4), Some(4));
        assert_eq!(input.utf16_to_char_index(5), None);
    }

    #[test]
    fn utf16_to_char_index_handles_astral_characters() {
        // An astral character (emoji) occupies two UTF-16 units.
        let input = input("你🙂");
        assert_eq!(input.utf16_to_char_index(0), Some(0));
        assert_eq!(input.utf16_to_char_index(1), Some(1));
        // Offsets inside a surrogate pair have no valid character boundary.
        assert_eq!(input.utf16_to_char_index(2), None);
        assert_eq!(input.utf16_to_char_index(3), Some(2));
    }
}
