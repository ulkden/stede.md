// SPDX-License-Identifier: GPL-3.0-or-later
//! The view model: everything a front end needs to draw one frame.
//!
//! Front ends draw this and nothing else. They never reach into the core's
//! state, so the terminal and a native window draw the same thing.

use crate::doc::LineKind;
use crate::layout::{PaneId, PaneRect};
use crate::modal::Modal;
use crate::tree::TreeRow;

/// Rows at the bottom of the screen for the status line.
pub const STATUS_ROWS: u16 = 1;

/// One frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View<'a> {
    /// Panes in screen order. Panes that are off are not listed.
    pub panes: Vec<PaneView>,
    /// The file tree pane's content.
    pub tree: TreeView,
    /// The raw pane's content.
    pub raw: RawView<'a>,
    /// The rendered pane's content.
    pub rendered: RenderedView<'a>,
    /// The search in force: light its matches in both reader panes.
    pub search: Option<SearchView<'a>>,
    /// Open modals, bottom first. Draw them all, top last.
    pub modals: Vec<&'a Modal>,
    /// The key bound to `modal.close`, for modal footers ("Esc to close").
    pub close_key: Option<String>,
    pub status: StatusView,
}

/// One pane: where it goes and how to mark it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneView {
    pub id: PaneId,
    pub title: &'static str,
    pub rect: PaneRect,
    pub focused: bool,
}

/// The status line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusView {
    /// The search pattern being typed, with its `/` or `?`: show it in place
    /// of the rest of the status line, with a cursor after it.
    pub prompt: Option<String>,
    /// The mode name, such as `normal`.
    pub mode: &'static str,
    /// Keys typed toward a binding, such as `3<C-w>`.
    pub pending: String,
    /// The last message or error, if any.
    pub message: Option<String>,
    /// The file being viewed, if any.
    pub file: Option<String>,
}

/// What the raw pane shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawView<'a> {
    /// The file's name, for the pane title.
    pub title: Option<String>,
    /// Lines from the top of the view, as many as fit unwrapped: each its
    /// 1-based number, text and kind. Draw them in order, wrapping long ones,
    /// until the pane is full.
    pub lines: Vec<(usize, &'a str, LineKind)>,
    /// Wrapped rows of the first line already scrolled past: start drawing it
    /// from this row.
    pub first_row: usize,
    /// The number column's width (0 for none) and the text's width, in
    /// cells: draw with these so rows break where scrolling counts them.
    pub gutter: usize,
    pub text_width: usize,
    /// Lines in the whole file, for the number column's width.
    pub total: usize,
    pub line_numbers: bool,
    /// What to show instead of text: loading, an error, or no file open.
    pub message: Option<String>,
    /// Changes when the document is loaded again.
    pub generation: u64,
}

/// The search in force.
#[derive(Clone, Copy, Debug)]
pub struct SearchView<'a> {
    /// Its pattern, compiled: find the matches in each row drawn.
    pub regex: &'a regex::Regex,
    /// The line (1-based) of the current match, drawn brighter.
    pub current_line: Option<usize>,
    /// The current match's place and the count, such as `(3, 12)`.
    pub position: Option<(usize, usize)>,
}

impl PartialEq for SearchView<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.regex.as_str() == other.regex.as_str()
            && self.current_line == other.current_line
            && self.position == other.position
    }
}

impl Eq for SearchView<'_> {}

/// What the rendered pane shows: the render model, from a block and a row
/// within it. The front end lays the model out and reports block heights
/// back (`App::set_rendered_layout`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderedView<'a> {
    pub model: Option<&'a crate::render::RenderModel>,
    /// Changes when the document is loaded again: lay it out again.
    pub generation: u64,
    /// The top block and how many of its rows are scrolled past.
    pub position: (usize, usize),
    /// Whether the core has a layout for this document yet.
    pub laid_out: bool,
}

/// What the file tree pane shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreeView {
    /// The root folder's name, for the pane title.
    pub root: Option<String>,
    /// Every visible row; draw from `scroll`.
    pub rows: Vec<TreeRow>,
    pub selected: Option<usize>,
    /// The first row to draw, keeping the selection on screen.
    pub scroll: usize,
    /// The root's listing hasn't arrived yet.
    pub loading: bool,
    /// Why the root couldn't be listed.
    pub error: Option<String>,
}
