// SPDX-License-Identifier: GPL-3.0-or-later
//! The contract between the core and its background services.
//!
//! The core never touches the file system on the input loop. It asks for
//! work with a [`Request`], which the front end hands to `stede-services`;
//! results come back as a [`ServiceEvent`] the front end passes to
//! `App::handle_event`.

use std::path::PathBuf;

use crate::config::{ConfigError, Layer};

/// One entry in a folder listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// Work the core asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// List a folder's entries (respecting `.gitignore`), and watch it.
    ListDir(PathBuf),
    /// The user opened a file. Showing it in the panes comes with the
    /// reader panes (phase 1 step 6).
    OpenFile(PathBuf),
}

/// What background work reports back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceEvent {
    /// A folder's entries, sorted folders first, or why it couldn't be read.
    Listed {
        dir: PathBuf,
        result: Result<Vec<Entry>, String>,
    },
    /// These folders changed on disk.
    Changed(Vec<PathBuf>),
    /// A config file changed on disk (or its folder appeared). The services
    /// read every layer again, off the input loop.
    ConfigChanged {
        layers: Vec<Layer>,
        errors: Vec<ConfigError>,
    },
}
