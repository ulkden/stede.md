// SPDX-License-Identifier: GPL-3.0-or-later
//! Named actions and the one registry that holds them.
//!
//! Every key press resolves to an [`Action`] such as `pane.focus_right` or
//! `pane.toggle tree`. Built-in actions, user commands and (later) plugin
//! commands all live in the same [`Registry`], and every entry carries a
//! one-line description for the key cheat sheet.

use std::collections::BTreeMap;
use std::fmt;

use crate::keymap::Mode;

/// An action to run: a registered name plus optional arguments.
///
/// Written as text it is the name followed by space-separated arguments, as in
/// keymap config: `"pane.toggle tree"`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Action {
    pub name: String,
    pub args: Vec<String>,
}

impl Action {
    /// An action with no arguments.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            args: Vec::new(),
        }
    }

    /// Parses `"name arg1 arg2"`. Returns `None` for an empty string.
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.split_whitespace();
        let name = parts.next()?.to_owned();
        Some(Self {
            name,
            args: parts.map(str::to_owned).collect(),
        })
    }

    /// The first argument, if any.
    pub fn arg(&self) -> Option<&str> {
        self.args.first().map(String::as_str)
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)?;
        for arg in &self.args {
            write!(f, " {arg}")?;
        }
        Ok(())
    }
}

/// Where a registered action comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Builtin,
    User,
    Plugin,
}

/// A registered action: its name, a one-line description, and its source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionSpec {
    pub name: String,
    pub description: String,
    pub source: Source,
    /// The modes this action can run in, and so be bound in.
    pub modes: &'static [Mode],
}

/// Why an action could not be registered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegisterError {
    /// Names are `group.verb`: lowercase ASCII letters, digits and `_`, with at
    /// least one `.` separating non-empty parts.
    BadName(String),
    /// The description is empty or runs over more than one line.
    BadDescription(String),
    /// The name is already registered.
    Duplicate(String),
}

impl fmt::Display for RegisterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadName(n) => write!(
                f,
                "action name {n:?} must look like `group.verb` (lowercase letters, digits, `_`)"
            ),
            Self::BadDescription(n) => {
                write!(f, "action {n:?} needs a one-line description")
            }
            Self::Duplicate(n) => write!(f, "action {n:?} is already registered"),
        }
    }
}

impl std::error::Error for RegisterError {}

/// Every action the editor knows, keyed by name.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    actions: BTreeMap<String, ActionSpec>,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry with every built-in action.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        for &(name, description, modes) in BUILTINS {
            registry
                .register_in(name, description, Source::Builtin, modes)
                .expect("built-in actions are valid and unique");
        }
        registry
    }

    /// Adds an action that runs in the editor's modes ([`Mode::EDITOR`]).
    pub fn register(
        &mut self,
        name: &str,
        description: &str,
        source: Source,
    ) -> Result<(), RegisterError> {
        self.register_in(name, description, source, &Mode::EDITOR)
    }

    /// Adds an action that runs only in `modes`.
    pub fn register_in(
        &mut self,
        name: &str,
        description: &str,
        source: Source,
        modes: &'static [Mode],
    ) -> Result<(), RegisterError> {
        if !valid_name(name) {
            return Err(RegisterError::BadName(name.to_owned()));
        }
        let description = description.trim();
        // One line, and nothing that moves the cursor when drawn (`\r`, tabs).
        if description.is_empty() || description.chars().any(char::is_control) {
            return Err(RegisterError::BadDescription(name.to_owned()));
        }
        if self.actions.contains_key(name) {
            return Err(RegisterError::Duplicate(name.to_owned()));
        }
        self.actions.insert(
            name.to_owned(),
            ActionSpec {
                name: name.to_owned(),
                description: description.to_owned(),
                source,
                modes,
            },
        );
        Ok(())
    }

    /// Checks an action's arguments, so a bad one is caught where it's bound
    /// (with its config file and line) rather than when the key is pressed.
    /// Built-ins say what they take; user and plugin commands take anything.
    pub fn check_args(&self, action: &Action) -> Result<(), String> {
        let Some(spec) = self.get(&action.name) else {
            return Err(format!("unknown action {:?}", action.name));
        };
        if spec.source != Source::Builtin {
            return Ok(());
        }
        match (action.name.as_str(), action.args.as_slice()) {
            ("pane.toggle", [pane]) => pane.parse::<crate::layout::PaneId>().map(|_| ()),
            ("pane.toggle", _) => Err("pane.toggle takes one pane: raw, rendered or tree".into()),
            (_, []) => Ok(()),
            (name, _) => Err(format!("{name} takes no arguments")),
        }
    }

    /// Looks up an action by name.
    pub fn get(&self, name: &str) -> Option<&ActionSpec> {
        self.actions.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.actions.contains_key(name)
    }

    /// Every action, sorted by name.
    pub fn iter(&self) -> impl Iterator<Item = &ActionSpec> {
        self.actions.values()
    }

    pub fn len(&self) -> usize {
        self.actions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }
}

fn valid_name(name: &str) -> bool {
    let parts: Vec<&str> = name.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}

/// Built-in actions: name, one-line description, and the modes they run in.
///
/// An action is listed here only once something can run it (the core, or a
/// front end for its own screens such as `probe.*`), so the cheat sheet never
/// offers a key that does nothing.
const EDITOR: &[Mode] = &Mode::EDITOR;
const PROBE: &[Mode] = &[Mode::Probe];

const BUILTINS: &[(&str, &str, &[Mode])] = &[
    ("app.quit", "Quit STEDE.md", EDITOR),
    (
        "config.problems",
        "Show the config problems found at start",
        EDITOR,
    ),
    ("help.keys", "Open the key cheat sheet", EDITOR),
    ("modal.close", "Close the top modal", EDITOR),
    ("modal.close_all", "Close every open modal", EDITOR),
    ("modal.reopen_last", "Reopen the last output modal", EDITOR),
    ("modal.scroll_down", "Scroll the top modal down", EDITOR),
    ("modal.scroll_up", "Scroll the top modal up", EDITOR),
    (
        "pane.focus_left",
        "Move focus to the pane on the left, opening it if folded",
        EDITOR,
    ),
    (
        "pane.focus_right",
        "Move focus to the pane on the right, opening it if folded",
        EDITOR,
    ),
    (
        "pane.focus_one",
        "Fold every pane but this one; again to restore the layout",
        EDITOR,
    ),
    (
        "pane.toggle",
        "Fold or open a pane: raw, rendered or tree",
        EDITOR,
    ),
    (
        "tree.down",
        "Select the next entry in the file tree",
        EDITOR,
    ),
    (
        "tree.up",
        "Select the previous entry in the file tree",
        EDITOR,
    ),
    (
        "tree.top",
        "Select the first entry in the file tree",
        EDITOR,
    ),
    (
        "tree.bottom",
        "Select the last entry in the file tree",
        EDITOR,
    ),
    (
        "tree.open",
        "Open the selected folder or file; on an open folder, close it",
        EDITOR,
    ),
    (
        "tree.close",
        "Close the selected folder, or go to its parent",
        EDITOR,
    ),
    (
        "probe.quit",
        "Leave the terminal probe (stede --probe)",
        PROBE,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display() {
        let a = Action::parse("  pane.toggle   tree ").unwrap();
        assert_eq!(a.name, "pane.toggle");
        assert_eq!(a.args, vec!["tree"]);
        assert_eq!(a.arg(), Some("tree"));
        assert_eq!(a.to_string(), "pane.toggle tree");
        assert_eq!(Action::parse("   "), None);
    }

    #[test]
    fn builtins_are_valid() {
        let r = Registry::with_builtins();
        assert_eq!(r.len(), BUILTINS.len());
        assert!(r.iter().all(|s| s.source == Source::Builtin));
        assert!(r.contains("app.quit"));
    }

    #[test]
    fn names_are_checked() {
        let mut r = Registry::new();
        for bad in [
            "quit",
            "App.quit",
            "app.",
            ".quit",
            "app..quit",
            "app.qu it",
        ] {
            assert_eq!(
                r.register(bad, "x", Source::User),
                Err(RegisterError::BadName(bad.into())),
                "{bad}"
            );
        }
        assert!(r.register("ai.tighten_2", "Tighten", Source::User).is_ok());
        assert_eq!(
            r.register("ai.tighten_2", "Again", Source::User),
            Err(RegisterError::Duplicate("ai.tighten_2".into()))
        );
    }

    #[test]
    fn descriptions_are_one_line() {
        let mut r = Registry::new();
        assert!(matches!(
            r.register("a.b", " ", Source::User),
            Err(RegisterError::BadDescription(_))
        ));
        assert!(matches!(
            r.register("a.b", "one\ntwo", Source::User),
            Err(RegisterError::BadDescription(_))
        ));
        assert!(matches!(
            r.register("a.b", "one\rtwo", Source::User),
            Err(RegisterError::BadDescription(_))
        ));
        // A trailing newline trims away: the text is still one line.
        assert!(r.register("a.b", "One line\n", Source::User).is_ok());
        assert_eq!(r.get("a.b").unwrap().description, "One line");
    }
}
