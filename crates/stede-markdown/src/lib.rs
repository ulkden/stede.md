// SPDX-License-Identifier: GPL-3.0-or-later
//! The Markdown document type for STEDE.md.
//!
//! comrak builds the full tree with source positions, and [`render`] turns it
//! into the [`RenderModel`] the rendered pane draws; callers run it off the
//! input loop (on a worker thread). tree-sitter-md, for raw-pane highlighting
//! and text objects, arrives with the reader panes. No UI code here.

mod convert;
mod flavor;

/// The render model's types, which live in the core (every crate shares them).
pub use stede_core::render as model;

pub use convert::{MAX_CONTAINERS, comrak_options, line_count, render};
pub use flavor::Flavor;
pub use model::RenderModel;
