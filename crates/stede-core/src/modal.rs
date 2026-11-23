// SPDX-License-Identifier: GPL-3.0-or-later
//! The modal stack: prompts and outputs that open over the panes.
//!
//! One close key closes the top modal; `close_all` closes them all. The last
//! output modal can be reopened, so a lint list or the key cheat sheet isn't
//! lost by closing it. The stack lives in the core so every front end draws
//! the same modals with the same keys.

/// What kind of modal this is. It decides which keys work inside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalKind {
    /// Type a value, Enter to confirm.
    Input,
    /// Yes or no.
    Confirm,
    /// Type to filter a list, Enter to choose.
    Picker,
    /// Read-only results: the cheat sheet, lint lists, command output.
    Output,
}

/// A table row in an output modal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub cells: Vec<String>,
}

/// A titled group of rows, such as one mode's bindings in the cheat sheet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub rows: Vec<Row>,
}

/// What a modal shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModalBody {
    /// Lines of plain text.
    Text(Vec<String>),
    /// Column headers and titled groups of rows.
    Table {
        headers: Vec<String>,
        sections: Vec<Section>,
    },
}

/// One modal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Modal {
    pub kind: ModalKind,
    pub title: String,
    pub body: ModalBody,
    /// First visible line, for long content.
    pub scroll: usize,
}

impl Modal {
    pub fn output(title: impl Into<String>, body: ModalBody) -> Self {
        Self {
            kind: ModalKind::Output,
            title: title.into(),
            body,
            scroll: 0,
        }
    }
}

/// Open modals, top last.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModalStack {
    stack: Vec<Modal>,
    last_output: Option<Modal>,
}

impl ModalStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens a modal on top of any others.
    pub fn open(&mut self, modal: Modal) {
        self.stack.push(modal);
    }

    /// Closes the top modal. Returns false if none was open.
    pub fn close(&mut self) -> bool {
        match self.stack.pop() {
            Some(modal) => {
                self.remember(modal);
                true
            }
            None => false,
        }
    }

    /// Closes every modal. Returns false if none was open.
    ///
    /// The output modal remembered for `reopen_last` is the bottom-most one:
    /// the one the user opened on purpose before stacking others on it.
    pub fn close_all(&mut self) -> bool {
        if self.stack.is_empty() {
            return false;
        }
        for modal in std::mem::take(&mut self.stack).into_iter().rev() {
            self.remember(modal);
        }
        true
    }

    /// Reopens the last output modal that was closed. Returns false if there
    /// is none.
    pub fn reopen_last(&mut self) -> bool {
        match self.last_output.take() {
            Some(modal) => {
                self.stack.push(modal);
                true
            }
            None => false,
        }
    }

    /// The top modal, which takes keys while open.
    pub fn top(&self) -> Option<&Modal> {
        self.stack.last()
    }

    pub fn top_mut(&mut self) -> Option<&mut Modal> {
        self.stack.last_mut()
    }

    /// Open modals, bottom first.
    pub fn iter(&self) -> impl Iterator<Item = &Modal> {
        self.stack.iter()
    }

    pub fn is_open(&self) -> bool {
        !self.stack.is_empty()
    }

    pub fn len(&self) -> usize {
        self.stack.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// Keeps the most recently closed output modal for `reopen_last`.
    fn remember(&mut self, modal: Modal) {
        if modal.kind == ModalKind::Output {
            self.last_output = Some(modal);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(title: &str) -> Modal {
        Modal::output(title, ModalBody::Text(vec![title.to_owned()]))
    }

    #[test]
    fn close_top_only() {
        let mut s = ModalStack::new();
        s.open(out("a"));
        s.open(out("b"));
        assert!(s.close());
        assert_eq!(s.top().unwrap().title, "a");
        assert!(s.close());
        assert!(!s.close());
    }

    #[test]
    fn close_all() {
        let mut s = ModalStack::new();
        assert!(!s.close_all());
        s.open(out("a"));
        s.open(out("b"));
        assert!(s.close_all());
        assert!(!s.is_open());
    }

    #[test]
    fn reopen_last_output() {
        let mut s = ModalStack::new();
        assert!(!s.reopen_last());
        s.open(out("keys"));
        s.close();
        assert!(s.reopen_last());
        assert_eq!(s.top().unwrap().title, "keys");
        // Reopening hands the modal back; it isn't duplicated.
        s.close();
        assert!(s.reopen_last());
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn reopen_gives_the_most_recent_close() {
        let mut s = ModalStack::new();
        s.open(out("old"));
        s.close();
        s.open(out("a"));
        s.open(out("b"));
        s.close();
        s.close();
        s.reopen_last();
        assert_eq!(s.top().unwrap().title, "a");
        s.close();
        s.open(out("c"));
        s.open(out("d"));
        s.close();
        s.reopen_last();
        assert_eq!(s.top().unwrap().title, "d", "closed with c still open");
    }

    #[test]
    fn close_all_keeps_the_bottom_output() {
        let mut s = ModalStack::new();
        s.open(out("lint"));
        s.open(out("keys"));
        s.close_all();
        s.reopen_last();
        assert_eq!(s.top().unwrap().title, "lint");
    }
}
