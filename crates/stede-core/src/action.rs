// SPDX-License-Identifier: GPL-3.0-or-later
//! Named actions and the one registry that holds them.
//!
//! Every key press resolves to an [`Action`] such as `pane.focus_right` or
//! `pane.toggle tree`. Built-in actions, user commands and (later) plugin
//! commands all live in the same [`Registry`], and every entry carries a
//! one-line description for the key cheat sheet.

use std::collections::BTreeMap;
use std::fmt;

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
        for (name, description) in BUILTINS {
            registry
                .register(name, description, Source::Builtin)
                .expect("built-in actions are valid and unique");
        }
        registry
    }

    /// Adds an action.
    pub fn register(
        &mut self,
        name: &str,
        description: &str,
        source: Source,
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
            },
        );
        Ok(())
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

/// Built-in actions: name and one-line description.
///
/// An action is listed here only once the core can run it, so the cheat sheet
/// never offers a key that does nothing.
const BUILTINS: &[(&str, &str)] = &[
    ("app.quit", "Quit STEDE.md"),
    ("help.keys", "Open the key cheat sheet"),
    ("modal.close", "Close the top modal"),
    ("modal.close_all", "Close every open modal"),
    ("modal.reopen_last", "Reopen the last output modal"),
    ("modal.scroll_down", "Scroll the top modal down"),
    ("modal.scroll_up", "Scroll the top modal up"),
    (
        "pane.focus_left",
        "Move focus to the pane on the left, opening it if folded",
    ),
    (
        "pane.focus_right",
        "Move focus to the pane on the right, opening it if folded",
    ),
    (
        "pane.focus_one",
        "Fold every pane but this one; again to restore the layout",
    ),
    ("pane.toggle", "Fold or open a pane: raw, rendered or tree"),
    ("probe.quit", "Leave the terminal probe (stede --probe)"),
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
