// SPDX-License-Identifier: GPL-3.0-or-later
//! The contract between the core and its background services.
//!
//! The core never touches the file system on the input loop. It asks for
//! work with a [`Request`], which the front end hands to `stede-services`;
//! results come back as a [`ServiceEvent`] the front end passes to
//! `App::handle_event`.

use std::path::PathBuf;

use crate::config::{ConfigError, Layer};
use crate::render::RenderModel;

/// One entry in a folder listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// What the file tree lists beyond the default (`[tree]` and the tree's
/// toggle keys). By default dotfiles and ignored files are left out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TreeFilter {
    /// Show dotfiles (`.git` itself stays hidden).
    pub show_hidden: bool,
    /// Show what `.gitignore`, `.ignore` and git's excludes leave out.
    pub show_ignored: bool,
}

/// Work the core asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// List a folder's entries (by the current [`TreeFilter`]), and watch it.
    ListDir(PathBuf),
    /// What later listings show. The core lists open folders again after it.
    TreeFilter(TreeFilter),
    /// Watch every folder under the tree's root that the tree would show
    /// (by the current [`TreeFilter`]), opened or not, so changes anywhere
    /// are seen. Capped, and skipped where watching means polling (WSL
    /// `/mnt`); folders the tree lists are watched either way.
    WatchTree(PathBuf),
    /// Stop watching a folder the tree no longer shows (it was removed).
    Unwatch(PathBuf),
    /// Read a file and build its render model, in a flavor (`None` for the
    /// default), off the input loop.
    OpenFile {
        path: PathBuf,
        flavor: Option<String>,
    },
}

/// A file, read and parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    pub path: PathBuf,
    pub text: String,
    pub model: RenderModel,
    /// Read as Markdown. Other text files become one code block.
    pub markdown: bool,
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
    /// Something to tell the user once, such as the watch cap being reached.
    Notice(String),
    /// A file was read and parsed, or why it couldn't be.
    Opened {
        path: PathBuf,
        result: Result<Document, String>,
    },
    /// A config file changed on disk (or its folder appeared). The services
    /// read every layer again, off the input loop.
    ConfigChanged {
        layers: Vec<Layer>,
        errors: Vec<ConfigError>,
    },
}
