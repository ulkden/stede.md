// SPDX-License-Identifier: GPL-3.0-or-later
//! The view model: everything a front end needs to draw one frame.
//!
//! Front ends draw this and nothing else. They never reach into the core's
//! state, so the terminal and a native window draw the same thing.

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
    /// The mode name, such as `normal`.
    pub mode: &'static str,
    /// Keys typed toward a binding, such as `3<C-w>`.
    pub pending: String,
    /// The last message or error, if any.
    pub message: Option<String>,
    /// The file being viewed, if any.
    pub file: Option<String>,
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
