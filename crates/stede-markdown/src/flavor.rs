// SPDX-License-Identifier: GPL-3.0-or-later
//! Markdown flavors: named sets of parser switches.
//!
//! GFM is the default. CommonMark turns every extension off. Obsidian-style
//! and MkDocs come later in the flavor order (`docs/architecture.md`).

use std::fmt;
use std::str::FromStr;

use comrak::Options;

/// A Markdown flavor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Flavor {
    /// GitHub Flavored Markdown: tables, task lists, strikethrough, autolinks,
    /// footnotes, alerts and front matter.
    #[default]
    Gfm,
    /// Plain CommonMark, no extensions.
    CommonMark,
}

impl Flavor {
    /// The name used in config (`flavor = "gfm"`).
    pub fn name(self) -> &'static str {
        match self {
            Flavor::Gfm => "gfm",
            Flavor::CommonMark => "commonmark",
        }
    }

    /// comrak's options for this flavor. Source positions are always kept:
    /// they drive scroll sync.
    pub(crate) fn options(self) -> Options<'static> {
        let mut options = Options::default();
        if self == Flavor::Gfm {
            let ext = &mut options.extension;
            ext.table = true;
            ext.tasklist = true;
            ext.strikethrough = true;
            ext.autolink = true;
            ext.footnotes = true;
            ext.alerts = true;
            ext.front_matter_delimiter = Some("---".to_owned());
        }
        options
    }
}

impl fmt::Display for Flavor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Flavor {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "gfm" => Ok(Flavor::Gfm),
            "commonmark" => Ok(Flavor::CommonMark),
            other => Err(format!(
                "unknown flavor {other:?}; expected gfm or commonmark"
            )),
        }
    }
}
