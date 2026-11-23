// SPDX-License-Identifier: GPL-3.0-or-later
//! The modal stack: prompts and outputs that open over the panes.
//!
//! One close key closes the top modal; `close_all` closes them all. The last
//! output modal can be reopened, so a lint list or the key cheat sheet isn't
//! lost by closing it. The stack lives in the core so every front end draws
//! the same modals with the same keys.

/// The most of the pane area a modal may cover, in percent. Front ends size
/// modals with [`max_size`] so the core's scrolling matches what is drawn.
pub const MAX_WIDTH_PCT: u16 = 80;
pub const MAX_HEIGHT_PCT: u16 = 70;

/// Rows a modal's border takes (top and bottom).
pub const BORDER_ROWS: u16 = 2;

/// The largest modal, in cells, for a pane area of this size.
pub fn max_size(width: u16, height: u16) -> (u16, u16) {
    let w = u32::from(width) * u32::from(MAX_WIDTH_PCT) / 100;
    let h = u32::from(height) * u32::from(MAX_HEIGHT_PCT) / 100;
    (w as u16, h as u16)
}

/// How many body lines fit in the largest modal for a pane area this tall.
pub fn max_body_rows(height: u16) -> usize {
    usize::from(max_size(0, height).1.saturating_sub(BORDER_ROWS))
}

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
    /// A stable identity for modals the core needs to find again, such as the
    /// key cheat sheet. Titles are for people and may repeat.
    pub id: Option<&'static str>,
    pub title: String,
    pub body: ModalBody,
    /// First visible line, for long content.
    pub scroll: usize,
}

impl Modal {
    pub fn output(title: impl Into<String>, body: ModalBody) -> Self {
        Self {
            kind: ModalKind::Output,
            id: None,
            title: title.into(),
            body,
            scroll: 0,
        }
    }

    /// How many lines the body takes when drawn: a table is its header line,
    /// then each section's title line followed by its rows. Front ends draw
    /// tables in exactly this shape, so scrolling agrees with what is shown.
    pub fn line_count(&self) -> usize {
        match &self.body {
            ModalBody::Text(lines) => lines.len(),
            ModalBody::Table { sections, .. } => {
                1 + sections.iter().map(|s| 1 + s.rows.len()).sum::<usize>()
            }
        }
    }

    /// Scrolls by `delta` lines. `visible` is how many body lines show; the
    /// scroll stops where the last line reaches the bottom, so scrolling back
    /// always moves the view.
    pub fn scroll_by(&mut self, delta: isize, visible: usize) {
        let max = self.line_count().saturating_sub(visible.max(1));
        self.scroll = self.scroll.saturating_add_signed(delta).min(max);
    }

    /// Sets the modal's identity.
    pub fn with_id(mut self, id: &'static str) -> Self {
        self.id = Some(id);
        self
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

    /// Keeps every modal's scroll, open or remembered, within what shows when
    /// `visible` body lines fit. Call it when the screen size changes.
    pub fn clamp_scroll(&mut self, visible: usize) {
        for modal in self.stack.iter_mut().chain(self.last_output.as_mut()) {
            modal.scroll_by(0, visible);
        }
    }

    /// Closes every open modal with this id, wherever it sits. Returns
    /// whether one was open.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.stack.len();
        self.stack.retain(|m| m.id != Some(id));
        self.stack.len() != before
    }

    /// Where the modal with this id sits, counting from the bottom.
    pub fn position(&self, id: &str) -> Option<usize> {
        self.stack.iter().position(|m| m.id == Some(id))
    }

    /// Closes modals from the top down to and including the one at `index`.
    pub fn close_through(&mut self, index: usize) {
        while self.stack.len() > index {
            self.close();
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
    fn line_count_and_scroll() {
        let mut m = Modal::output(
            "t",
            ModalBody::Table {
                headers: vec!["a".into()],
                sections: vec![
                    Section {
                        title: "s1".into(),
                        rows: vec![
                            Row {
                                cells: vec!["x".into()]
                            };
                            2
                        ],
                    },
                    Section {
                        title: "s2".into(),
                        rows: vec![Row {
                            cells: vec!["y".into()],
                        }],
                    },
                ],
            },
        );
        assert_eq!(m.line_count(), 1 + 3 + 2);
        m.scroll_by(-3, 2);
        assert_eq!(m.scroll, 0);
        m.scroll_by(100, 2);
        assert_eq!(
            m.scroll, 4,
            "6 lines, 2 visible: stops with the last line at the bottom"
        );
        m.scroll_by(-1, 2);
        assert_eq!(m.scroll, 3, "and the first press back moves the view");
        m.scroll_by(100, 10);
        assert_eq!(m.scroll, 0, "everything fits: no scroll");
    }

    #[test]
    fn size_limits() {
        assert_eq!(max_size(100, 40), (80, 28));
        assert_eq!(max_body_rows(40), 26);
        assert_eq!(max_body_rows(1), 0);
    }

    #[test]
    fn clamp_scroll_covers_open_and_remembered_modals() {
        let long = || {
            let mut m = Modal::output("t", ModalBody::Text(vec!["x".into(); 20]));
            m.scroll = 18;
            m
        };
        let mut s = ModalStack::new();
        s.open(long());
        s.close();
        s.open(long());
        s.clamp_scroll(10);
        assert_eq!(s.top().unwrap().scroll, 10);
        s.close_all();
        s.reopen_last();
        assert_eq!(s.top().unwrap().scroll, 10);
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
