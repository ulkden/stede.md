// SPDX-License-Identifier: GPL-3.0-or-later
//! Config: TOML in layers, last wins.
//!
//! The layers, in order: built-in defaults, the user file
//! (`$XDG_CONFIG_HOME/stede/config.toml`, else `~/.config/stede/config.toml`),
//! then the project's `.stede/config.toml` and `.stede/local.toml`, found by
//! walking up from the opened path. (Profiles and session `:set` come later.)
//!
//! Each entry is checked on its own. A bad entry is reported with its file,
//! line and key and skipped; everything else still applies. Settings the
//! design names but that aren't built yet get a note, not an error, so a
//! config written ahead of time doesn't look broken.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use toml_edit::{Document, Item, TableLike};

use crate::key::{Key, parse_keys};
use crate::keymap::{Mode, Scope};
use crate::layout::{PaneId, TreeSide};
use crate::theme::{BUILTIN_THEMES, Color, Theme};

/// How serious a config problem is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// The entry was skipped.
    Error,
    /// The entry is fine but does nothing yet.
    Note,
}

/// A problem in a config file: where it is and what is wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError {
    pub severity: Severity,
    pub file: PathBuf,
    /// 1-based; 0 when the problem is the whole file (it couldn't be read).
    pub line: usize,
    /// The dotted key, such as `keys.normal."<leader>p"`; empty for the file.
    pub key: String,
    pub message: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.severity == Severity::Note {
            f.write_str("note: ")?;
        }
        write!(f, "{}", self.file.display())?;
        if self.line > 0 {
            write!(f, ":{}", self.line)?;
        }
        if !self.key.is_empty() {
            write!(f, ": {}", self.key)?;
        }
        write!(f, ": {}", self.message)
    }
}

/// Where a setting came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub file: PathBuf,
    pub line: usize,
    pub key: String,
}

impl Origin {
    /// A config error at this origin.
    pub fn error(&self, message: impl Into<String>) -> ConfigError {
        ConfigError {
            severity: Severity::Error,
            file: self.file.clone(),
            line: self.line,
            key: self.key.clone(),
            message: message.into(),
        }
    }
}

/// One config file's text.
#[derive(Clone, Debug)]
pub struct Layer {
    pub path: PathBuf,
    pub text: String,
}

/// A key binding from config: `keys` in Vim notation, `action` as
/// `"name args"`, or empty to unbind the keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingEntry {
    pub mode: Mode,
    pub scope: Scope,
    pub keys: String,
    pub action: String,
    pub origin: Origin,
}

/// The merged config.
#[derive(Clone, Debug)]
pub struct Config {
    pub leader: Key,
    pub key_timeout: Duration,
    /// Bindings in layer order; later ones win. Applied over the defaults.
    pub bindings: Vec<BindingEntry>,
    pub tree_side: TreeSide,
    /// The Markdown flavor, checked by the Markdown crate.
    pub flavor: Option<(String, Origin)>,
    pub theme: Theme,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            leader: Key::char(' '),
            key_timeout: crate::app::DEFAULT_KEY_TIMEOUT,
            bindings: Vec::new(),
            tree_side: TreeSide::Right,
            flavor: None,
            theme: Theme::default(),
        }
    }
}

/// Keys the design names that aren't built yet: accepted with a note.
const PLANNED_TOP: &[&str] = &[
    "editor",
    "modal",
    "help",
    "commands",
    "lint",
    "linters",
    "profile",
    "profiles",
    "templates",
];
const PLANNED_UI: &[&str] = &["animation", "font", "font_size"];

/// The tables under `[keys]`: which mode and scope each one binds in.
const KEY_TABLES: &[(&str, Mode, Scope)] = &[
    ("normal", Mode::Normal, Scope::Global),
    ("raw", Mode::Normal, Scope::Pane(PaneId::Raw)),
    ("rendered", Mode::Normal, Scope::Pane(PaneId::Rendered)),
    ("tree", Mode::Normal, Scope::Pane(PaneId::Tree)),
    ("modal", Mode::Normal, Scope::Modal),
    ("output", Mode::Normal, Scope::OutputModal),
    ("probe", Mode::Probe, Scope::Global),
];

/// The longest key timeout accepted, in milliseconds.
pub const MAX_KEY_TIMEOUT_MS: i64 = 10_000;

impl Config {
    /// Merges layers over the defaults, in order. Returns the config and every
    /// problem found; the config holds everything that was valid.
    pub fn from_layers(layers: &[Layer]) -> (Self, Vec<ConfigError>) {
        let mut config = Config::default();
        let mut errors = Vec::new();
        let mut theme_name = "dark".to_owned();
        let mut colors: Vec<(String, Color)> = Vec::new();
        for layer in layers {
            let mut parser = LayerParser {
                layer,
                config: &mut config,
                errors: &mut errors,
                theme_name: &mut theme_name,
                colors: &mut colors,
            };
            parser.parse();
        }
        let mut theme = Theme::builtin(&theme_name).expect("theme names are checked when read");
        for (slot, color) in colors {
            theme
                .set(&slot, color)
                .expect("slots are checked when read");
        }
        config.theme = theme;
        (config, errors)
    }
}

struct LayerParser<'a> {
    layer: &'a Layer,
    config: &'a mut Config,
    errors: &'a mut Vec<ConfigError>,
    theme_name: &'a mut String,
    colors: &'a mut Vec<(String, Color)>,
}

/// Quotes a key for a dotted path when it isn't a bare TOML key.
fn path_part(key: &str) -> String {
    if !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        key.to_owned()
    } else {
        format!("{key:?}")
    }
}

impl LayerParser<'_> {
    fn line_at(&self, offset: usize) -> usize {
        let end = offset.min(self.layer.text.len());
        self.layer.text.as_bytes()[..end]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
            + 1
    }

    /// The origin of `key` in `table`: the key's own line when known.
    fn origin(&self, table: &dyn TableLike, key: &str, path: &str) -> Origin {
        let span = table
            .key(key)
            .and_then(|k| k.span())
            .or_else(|| table.get(key).and_then(Item::span));
        Origin {
            file: self.layer.path.clone(),
            line: span.map_or(0, |s| self.line_at(s.start)),
            key: path.to_owned(),
        }
    }

    fn error(&mut self, origin: &Origin, message: impl Into<String>) {
        self.errors.push(origin.error(message));
    }

    fn note(&mut self, origin: &Origin, message: impl Into<String>) {
        let mut e = origin.error(message);
        e.severity = Severity::Note;
        self.errors.push(e);
    }

    fn parse(&mut self) {
        let doc = match Document::parse(self.layer.text.clone()) {
            Ok(doc) => doc,
            Err(e) => {
                let line = e.span().map_or(0, |s| self.line_at(s.start));
                let message = e.message().trim().to_owned();
                self.errors.push(ConfigError {
                    severity: Severity::Error,
                    file: self.layer.path.clone(),
                    line,
                    key: String::new(),
                    message: format!("not valid TOML: {message}"),
                });
                return;
            }
        };
        let root = doc.as_table();
        for (key, item) in root.iter() {
            let origin = self.origin(root, key, &path_part(key));
            match key {
                "keys" => self.keys(item, &origin),
                "ui" => self.ui(item, &origin),
                "markdown" => self.markdown(item, &origin),
                "theme" => self.theme(item, &origin),
                k if PLANNED_TOP.contains(&k) => {
                    self.note(&origin, "not built yet; this setting does nothing for now")
                }
                _ => self.error(&origin, "unknown key"),
            }
        }
    }

    fn table<'i>(&mut self, item: &'i Item, origin: &Origin) -> Option<&'i dyn TableLike> {
        let table = item.as_table_like();
        if table.is_none() {
            self.error(
                origin,
                format!("expected a table, found {}", item.type_name()),
            );
        }
        table
    }

    fn string<'i>(&mut self, item: &'i Item, origin: &Origin) -> Option<&'i str> {
        let s = item.as_str();
        if s.is_none() {
            self.error(
                origin,
                format!("expected a string, found {}", item.type_name()),
            );
        }
        s
    }

    fn keys(&mut self, item: &Item, origin: &Origin) {
        let Some(table) = self.table(item, origin) else {
            return;
        };
        for (key, item) in table.iter() {
            let origin = self.origin(table, key, &format!("keys.{}", path_part(key)));
            match key {
                "leader" => {
                    let Some(value) = self.string(item, &origin) else {
                        continue;
                    };
                    match parse_leader(value) {
                        Ok(leader) => self.config.leader = leader,
                        Err(e) => self.error(&origin, e),
                    }
                }
                "preset" => match self.string(item, &origin) {
                    Some("vim") | None => {}
                    Some("plain") => self.note(&origin, "the plain preset isn't built yet; using vim"),
                    Some(other) => self.error(&origin, format!("unknown preset {other:?}; expected vim")),
                },
                "timeout_ms" => match item.as_integer() {
                    Some(ms) if (0..=MAX_KEY_TIMEOUT_MS).contains(&ms) => {
                        self.config.key_timeout = Duration::from_millis(ms as u64);
                    }
                    Some(_) => self.error(&origin, format!("must be 0 to {MAX_KEY_TIMEOUT_MS}")),
                    None => self.error(
                        &origin,
                        format!("expected an integer, found {}", item.type_name()),
                    ),
                },
                name => match KEY_TABLES.iter().find(|(n, ..)| *n == name) {
                    Some(&(_, mode, scope)) => self.bindings(item, &origin, mode, scope),
                    None => self.error(
                        &origin,
                        "unknown key; binding tables are normal, raw, rendered, tree, modal, output and probe",
                    ),
                },
            }
        }
    }

    fn bindings(&mut self, item: &Item, origin: &Origin, mode: Mode, scope: Scope) {
        let Some(table) = self.table(item, origin) else {
            return;
        };
        for (keys, item) in table.iter() {
            let origin = self.origin(table, keys, &format!("{}.{}", origin.key, path_part(keys)));
            let Some(action) = self.string(item, &origin) else {
                continue;
            };
            self.config.bindings.push(BindingEntry {
                mode,
                scope,
                keys: keys.to_owned(),
                action: action.trim().to_owned(),
                origin,
            });
        }
    }

    fn ui(&mut self, item: &Item, origin: &Origin) {
        let Some(table) = self.table(item, origin) else {
            return;
        };
        for (key, item) in table.iter() {
            let origin = self.origin(table, key, &format!("ui.{}", path_part(key)));
            match key {
                "tree_side" => match self.string(item, &origin) {
                    Some("left") => self.config.tree_side = TreeSide::Left,
                    Some("right") => self.config.tree_side = TreeSide::Right,
                    Some(other) => {
                        self.error(&origin, format!("{other:?}: expected left or right"))
                    }
                    None => {}
                },
                k if PLANNED_UI.contains(&k) => {
                    self.note(&origin, "not built yet; this setting does nothing for now")
                }
                _ => self.error(&origin, "unknown key"),
            }
        }
    }

    fn markdown(&mut self, item: &Item, origin: &Origin) {
        let Some(table) = self.table(item, origin) else {
            return;
        };
        for (key, item) in table.iter() {
            let origin = self.origin(table, key, &format!("markdown.{}", path_part(key)));
            match key {
                "flavor" => {
                    if let Some(flavor) = self.string(item, &origin) {
                        self.config.flavor = Some((flavor.to_owned(), origin));
                    }
                }
                _ => self.error(&origin, "unknown key"),
            }
        }
    }

    fn theme(&mut self, item: &Item, origin: &Origin) {
        let Some(table) = self.table(item, origin) else {
            return;
        };
        for (key, item) in table.iter() {
            let origin = self.origin(table, key, &format!("theme.{}", path_part(key)));
            match key {
                "name" => match self.string(item, &origin) {
                    Some(name) if BUILTIN_THEMES.contains(&name) => {
                        *self.theme_name = name.to_owned()
                    }
                    Some(name) => self.error(
                        &origin,
                        format!(
                            "unknown theme {name:?}; built in: {}",
                            BUILTIN_THEMES.join(", ")
                        ),
                    ),
                    None => {}
                },
                "colors" => self.colors(item, &origin),
                _ => self.error(&origin, "unknown key"),
            }
        }
    }

    fn colors(&mut self, item: &Item, origin: &Origin) {
        let Some(table) = self.table(item, origin) else {
            return;
        };
        for (slot, item) in table.iter() {
            let origin = self.origin(table, slot, &format!("theme.colors.{}", path_part(slot)));
            let Some(value) = self.string(item, &origin) else {
                continue;
            };
            if Theme::default().set(slot, Color::Default).is_err() {
                self.error(&origin, "unknown theme slot");
                continue;
            }
            match value.parse::<Color>() {
                Ok(color) => self.colors.push((slot.to_owned(), color)),
                Err(e) => self.error(&origin, e),
            }
        }
    }
}

/// The leader: `space` or one key in Vim notation (`,`, `\`, `<Space>`).
fn parse_leader(value: &str) -> Result<Key, String> {
    if value.eq_ignore_ascii_case("space") {
        return Ok(Key::char(' '));
    }
    match parse_keys(value, None)
        .map_err(|e| e.to_string())?
        .as_slice()
    {
        [key] => Ok(*key),
        _ => Err(format!("{value:?}: the leader must be a single key")),
    }
}

/// The user config file: `$XDG_CONFIG_HOME/stede/config.toml`, else
/// `$HOME/.config/stede/config.toml`. `env` looks up an environment variable.
pub fn user_config_path(env: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let base = env("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env("HOME")
                .filter(|v| !v.is_empty())
                .map(|h| Path::new(&h).join(".config"))
        })?;
    Some(base.join(crate::SHORT_NAME).join("config.toml"))
}

/// The project's `.stede/` folder: the nearest one at or above `start` (a
/// folder, or a file's folder).
pub fn project_dir(start: &Path) -> Option<PathBuf> {
    let start = if start.is_file() {
        start.parent()?
    } else {
        start
    };
    start
        .ancestors()
        .map(|dir| dir.join(".stede"))
        .find(|candidate| candidate.is_dir())
}

/// Config files in layer order, whether they exist or not.
pub fn layer_paths(start: &Path, env: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = user_config_path(env).into_iter().collect();
    if let Some(dir) = project_dir(start) {
        paths.push(dir.join("config.toml"));
        paths.push(dir.join("local.toml"));
    }
    paths
}

/// Reads the files that exist. Missing files are fine; other read errors
/// (permissions, a folder in the way) are reported.
pub fn read_layers(paths: &[PathBuf]) -> (Vec<Layer>, Vec<ConfigError>) {
    let mut layers = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        match std::fs::read_to_string(path) {
            Ok(text) => layers.push(Layer {
                path: path.clone(),
                text,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => errors.push(ConfigError {
                severity: Severity::Error,
                file: path.clone(),
                line: 0,
                key: String::new(),
                message: format!("couldn't read it: {e}"),
            }),
        }
    }
    (layers, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(name: &str, text: &str) -> Layer {
        Layer {
            path: PathBuf::from(name),
            text: text.to_owned(),
        }
    }

    fn messages(errors: &[ConfigError]) -> Vec<String> {
        errors.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn defaults_without_layers() {
        let (c, errors) = Config::from_layers(&[]);
        assert!(errors.is_empty());
        assert_eq!(c.leader, Key::char(' '));
        assert_eq!(c.theme.name(), "dark");
        assert_eq!(c.tree_side, TreeSide::Right);
    }

    #[test]
    fn later_layers_win() {
        let (c, errors) = Config::from_layers(&[
            layer(
                "user.toml",
                "[ui]\ntree_side = \"left\"\n[keys]\nleader = \",\"\n",
            ),
            layer("project.toml", "[ui]\ntree_side = \"right\"\n"),
        ]);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(c.tree_side, TreeSide::Right);
        assert_eq!(c.leader, Key::char(','));
    }

    #[test]
    fn bindings_keep_their_origin() {
        let (c, errors) = Config::from_layers(&[layer(
            "c.toml",
            "\n[keys.normal]\n\"<leader>p\" = \"pane.toggle rendered\"\n\n[keys.tree]\no = \"\"\n",
        )]);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(c.bindings.len(), 2);
        let b = &c.bindings[0];
        assert_eq!((b.mode, b.scope), (Mode::Normal, Scope::Global));
        assert_eq!(b.origin.line, 3);
        assert_eq!(b.origin.key, "keys.normal.\"<leader>p\"");
        assert_eq!(c.bindings[1].scope, Scope::Pane(PaneId::Tree));
        assert_eq!(c.bindings[1].action, "");
    }

    #[test]
    fn errors_report_file_line_and_key_and_skip_only_the_bad_entry() {
        let (c, errors) = Config::from_layers(&[layer(
            "c.toml",
            "[ui]\ntree_side = \"top\"\nbogus = 1\n\n[keys]\ntimeout_ms = 99999\nleader = \"ab\"\n[keys.normal]\nx = 3\n\n[theme]\nname = \"neon\"\n[theme.colors]\n\"ui.accent\" = \"teal\"\n\"ui.nope\" = \"#fff\"\n\n[nope]\n",
        )]);
        assert_eq!(
            messages(&errors),
            vec![
                "c.toml:2: ui.tree_side: \"top\": expected left or right",
                "c.toml:3: ui.bogus: unknown key",
                "c.toml:6: keys.timeout_ms: must be 0 to 10000",
                "c.toml:7: keys.leader: \"ab\": the leader must be a single key",
                "c.toml:9: keys.normal.x: expected a string, found integer",
                "c.toml:12: theme.name: unknown theme \"neon\"; built in: dark, light",
                "c.toml:14: theme.colors.\"ui.accent\": \"teal\" isn't a color: use #rrggbb, default, or an ANSI name such as red or bright-blue",
                "c.toml:15: theme.colors.\"ui.nope\": unknown theme slot",
                "c.toml:17: nope: unknown key",
            ]
        );
        // The bad entries were skipped; defaults stand.
        assert_eq!(c.tree_side, TreeSide::Right);
        assert_eq!(c.theme.name(), "dark");
    }

    #[test]
    fn invalid_toml_reports_its_line() {
        let (_, errors) = Config::from_layers(&[layer("c.toml", "[ui]\ntree_side = \n")]);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, 2);
        assert!(
            errors[0].message.starts_with("not valid TOML"),
            "{}",
            errors[0]
        );
    }

    #[test]
    fn planned_settings_get_a_note() {
        let (_, errors) = Config::from_layers(&[layer(
            "c.toml",
            "[ui.animation]\nenabled = true\n[editor.line_numbers]\nmode = \"hybrid\"\n[keys]\npreset = \"plain\"\n",
        )]);
        assert!(
            errors.iter().all(|e| e.severity == Severity::Note),
            "{errors:?}"
        );
        assert_eq!(errors.len(), 3);
        assert!(
            errors[0]
                .to_string()
                .starts_with("note: c.toml:1: ui.animation")
        );
    }

    #[test]
    fn theme_name_and_colors() {
        let (c, errors) = Config::from_layers(&[
            layer("a.toml", "[theme.colors]\n\"ui.accent\" = \"#ff0000\"\n"),
            layer("b.toml", "[theme]\nname = \"light\"\n"),
        ]);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(c.theme.name(), "light");
        // Color overrides apply over whichever theme ends up chosen.
        assert_eq!(c.theme.get("ui.accent"), Color::Rgb(255, 0, 0));
    }

    #[test]
    fn inline_tables_work_too() {
        let (c, errors) = Config::from_layers(&[layer(
            "c.toml",
            "keys = { leader = \"\\\\\", normal = { \"gx\" = \"app.quit\" } }\n",
        )]);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(c.leader, Key::char('\\'));
        assert_eq!(c.bindings[0].keys, "gx");
    }

    #[test]
    fn user_config_path_honors_xdg() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                vars.iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(
            user_config_path(env(&[("XDG_CONFIG_HOME", "/x"), ("HOME", "/h")])),
            Some(PathBuf::from("/x/stede/config.toml"))
        );
        assert_eq!(
            user_config_path(env(&[("XDG_CONFIG_HOME", ""), ("HOME", "/h")])),
            Some(PathBuf::from("/h/.config/stede/config.toml"))
        );
        assert_eq!(user_config_path(env(&[])), None);
    }

    #[test]
    fn project_dir_walks_up() {
        let root = std::env::temp_dir().join(format!("stede-config-test-{}", std::process::id()));
        let deep = root.join("a/b");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(root.join(".stede")).unwrap();
        std::fs::write(deep.join("n.md"), "x").unwrap();
        assert_eq!(project_dir(&deep.join("n.md")), Some(root.join(".stede")));
        assert_eq!(project_dir(&deep), Some(root.join(".stede")));
        let paths = layer_paths(&deep, |_| None);
        assert_eq!(
            paths,
            vec![
                root.join(".stede/config.toml"),
                root.join(".stede/local.toml")
            ]
        );
        std::fs::write(
            root.join(".stede/config.toml"),
            "[ui]\ntree_side = \"left\"\n",
        )
        .unwrap();
        let (layers, errors) = read_layers(&paths);
        assert!(errors.is_empty());
        assert_eq!(layers.len(), 1, "local.toml is missing, which is fine");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
