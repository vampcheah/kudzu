use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    NewFile,
    NewFolder,
    Rename,
    Delete,
}

#[derive(Debug, Clone)]
pub struct Prompt {
    pub kind: PromptKind,
    pub buffer: String,
    /// Cursor position as a char (not byte) index into `buffer`.
    pub cursor: usize,
    /// For NewFile/NewFolder: the parent directory the new entry will be
    /// created in. For Rename/Delete: the full path of the entry being
    /// renamed or deleted.
    pub target: PathBuf,
}

impl Prompt {
    fn char_len(&self) -> usize {
        self.buffer.chars().count()
    }

    pub(super) fn byte_at(&self, char_idx: usize) -> usize {
        self.buffer
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.buffer.len())
    }

    pub(super) fn insert_char(&mut self, c: char) {
        let byte = self.byte_at(self.cursor);
        self.buffer.insert(byte, c);
        self.cursor += 1;
    }

    pub(super) fn delete_before(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_at(self.cursor - 1);
        let end = self.byte_at(self.cursor);
        self.buffer.replace_range(start..end, "");
        self.cursor -= 1;
    }

    pub(super) fn delete_at(&mut self) {
        let len = self.char_len();
        if self.cursor >= len {
            return;
        }
        let start = self.byte_at(self.cursor);
        let end = self.byte_at(self.cursor + 1);
        self.buffer.replace_range(start..end, "");
    }

    pub(super) fn delete_word_before(&mut self) {
        let chars: Vec<char> = self.buffer.chars().collect();
        let mut i = self.cursor;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        let start = self.byte_at(i);
        let end = self.byte_at(self.cursor);
        self.buffer.replace_range(start..end, "");
        self.cursor = i;
    }

    pub(super) fn kill_to_start(&mut self) {
        let end = self.byte_at(self.cursor);
        self.buffer.replace_range(0..end, "");
        self.cursor = 0;
    }

    pub(super) fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub(super) fn move_right(&mut self) {
        let len = self.char_len();
        if self.cursor < len {
            self.cursor += 1;
        }
    }

    pub(super) fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub(super) fn move_end(&mut self) {
        self.cursor = self.char_len();
    }
}
