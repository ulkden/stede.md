// SPDX-License-Identifier: GPL-3.0-or-later
//! STEDE.md editor core.
//!
//! Owns buffers, the Vim engine, keymaps, the command registry, the layout
//! model (panes, strips, modals) and config. It must never depend on a UI
//! crate: front ends send key events in and draw the view model that comes
//! back. See `docs/architecture.md`.

pub mod action;
pub mod app;
pub mod config;
pub mod doc;
pub mod io;
pub mod key;
pub mod keymap;
pub mod layout;
pub mod modal;
pub mod render;
pub mod theme;
pub mod tree;
pub mod view;

pub use app::{App, AppOptions, Handled};

/// The app's display name.
pub const APP_NAME: &str = "STEDE.md";

/// The short name used for the binary, crates and config paths.
pub const SHORT_NAME: &str = "stede";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_set() {
        assert_eq!(APP_NAME, "STEDE.md");
        assert_eq!(SHORT_NAME, "stede");
    }
}
