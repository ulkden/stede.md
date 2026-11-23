// SPDX-License-Identifier: GPL-3.0-or-later
//! The editor loop: key events in, actions run, a view model out.
//!
//! A front end owns the event loop and the clock. It sends each key press to
//! [`App::handle_key`]; if that reports a pending sequence, it calls
//! [`App::handle_timeout`] once [`App::key_timeout`] passes with no new key.
//! Then it draws [`App::view`].

use std::time::Duration;

use crate::action::{Action, Registry};
use crate::key::{Key, format_keys};
use crate::keymap::{Context, Keymap, Mode, Resolver, Scope, Step};
use crate::layout::{Layout, LayoutOptions, PaneId};
use crate::modal::{Modal, ModalBody, ModalStack, Row, Section};
use crate::view::{PaneView, STATUS_ROWS, StatusView, View};

/// Identifies the key cheat sheet among open modals, so its hotkey toggles it.
const CHEAT_SHEET_ID: &str = "help.keys";

/// How long to wait for the next key of an ambiguous sequence (Vim's `timeoutlen`).
pub const DEFAULT_KEY_TIMEOUT: Duration = Duration::from_millis(1000);

/// What a key press did, for the front end's loop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Handled {
    /// The view changed and should be drawn again.
    pub redraw: bool,
    /// A sequence is pending: call `handle_timeout` if no key comes in time.
    pub wait: bool,
}

/// How the app starts.
#[derive(Clone, Debug, Default)]
pub struct AppOptions {
    pub layout: LayoutOptions,
    /// The merged config. Its tree side, keys and timeout override `layout`
    /// and the defaults.
    pub config: crate::config::Config,
    /// Problems found while reading config files; shown in a modal at start.
    pub config_errors: Vec<crate::config::ConfigError>,
}

/// The editor state a front end drives.
#[derive(Debug)]
pub struct App {
    registry: Registry,
    keymap: Keymap,
    resolver: Resolver,
    layout: Layout,
    mode: Mode,
    message: Option<String>,
    key_timeout: Duration,
    quit: bool,
    /// The screen size the front end last reported, in cells.
    screen: (u16, u16),
}

impl App {
    pub fn new(options: AppOptions) -> Self {
        let registry = Registry::with_builtins();
        let (keymap, key_errors) = Keymap::from_config(&registry, &options.config);
        let layout = Layout::new(LayoutOptions {
            tree_side: options.config.tree_side,
            ..options.layout
        });
        let mut app = Self {
            registry,
            keymap,
            resolver: Resolver::new(),
            layout,
            mode: Mode::Normal,
            message: None,
            key_timeout: options.config.key_timeout,
            quit: false,
            screen: (80, 24),
        };
        let all: Vec<&crate::config::ConfigError> =
            options.config_errors.iter().chain(&key_errors).collect();
        if !all.is_empty() {
            use crate::config::Severity;
            let errors = all.iter().filter(|e| e.severity == Severity::Error).count();
            let notes = all.len() - errors;
            let reopen = app
                .keymap
                .keys_for(Mode::Normal, Scope::Global, "modal.reopen_last")
                .map(|k| format_keys(&k))
                .unwrap_or_else(|| "modal.reopen_last".into());
            let modal = Modal::output("Config problems", ModalBody::Text(problem_lines(&all)));
            if errors > 0 {
                // Errors mean config was skipped: show them at once.
                app.notify(format!(
                    "{errors} config error(s): {reopen} to reopen the list"
                ));
                app.layout.modals_mut().open(modal);
            } else {
                // Notes only (settings not built yet): don't get in the way.
                app.notify(format!(
                    "{notes} config setting(s) not built yet: {reopen} to see them"
                ));
                app.layout.modals_mut().stash(modal);
            }
        }
        app
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn modals(&self) -> &ModalStack {
        self.layout.modals()
    }

    /// True once `app.quit` has run; the front end should exit.
    pub fn should_quit(&self) -> bool {
        self.quit
    }

    pub fn key_timeout(&self) -> Duration {
        self.key_timeout
    }

    fn context(&self) -> Context {
        Context {
            mode: self.mode,
            pane: self.layout.focus(),
            modal: self.layout.modals().top().map(|m| m.kind),
        }
    }

    /// Tells the core the screen size, so actions that depend on it (modal
    /// scrolling) agree with what is drawn. Call it before each draw.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.screen = (width, height);
        // A bigger screen shows more lines, so the furthest useful scroll is
        // smaller: clamp now so the next `k` moves the view.
        let visible = crate::modal::max_body_rows(self.panes_height());
        self.layout.modals_mut().clamp_scroll(visible);
    }

    /// Shows a message in the status line until the next action.
    pub fn notify(&mut self, message: impl Into<String>) {
        self.message = Some(message.into());
    }

    /// Height of the pane area: the screen less the status line.
    fn panes_height(&self) -> u16 {
        self.screen.1.saturating_sub(STATUS_ROWS)
    }

    /// Handles one key press.
    pub fn handle_key(&mut self, key: Key) -> Handled {
        let ctx = self.context();
        let step = self.resolver.feed(&self.keymap, ctx, key);
        self.apply(step)
    }

    /// Call when [`key_timeout`](Self::key_timeout) passes after a pending key.
    pub fn handle_timeout(&mut self) -> Handled {
        let ctx = self.context();
        match self.resolver.timeout(&self.keymap, ctx) {
            Some(step) => self.apply(step),
            None => Handled {
                redraw: true,
                wait: false,
            },
        }
    }

    fn apply(&mut self, step: Step) -> Handled {
        match step {
            Step::Pending => Handled {
                redraw: true,
                wait: true,
            },
            // A count waits for its key with no timeout.
            Step::Count => Handled {
                redraw: true,
                wait: false,
            },
            Step::Cancelled => {
                self.message = None;
                Handled {
                    redraw: true,
                    wait: false,
                }
            }
            Step::Unbound(keys) => {
                self.message = Some(format!("{keys} is not bound"));
                Handled {
                    redraw: true,
                    wait: false,
                }
            }
            Step::Run { action, count } => {
                self.message = None;
                if let Err(e) = self.run(&action, count) {
                    self.message = Some(e);
                }
                Handled {
                    redraw: true,
                    wait: false,
                }
            }
        }
    }

    /// Runs a registered action. `count` repeats actions where that makes
    /// sense (focus moves); others ignore it.
    pub fn run(&mut self, action: &Action, count: Option<u32>) -> Result<(), String> {
        if !self.registry.contains(&action.name) {
            return Err(format!("unknown action {:?}", action.name));
        }
        let times = count.unwrap_or(1).max(1);
        match action.name.as_str() {
            "app.quit" => self.quit = true,
            "help.keys" => self.toggle_cheat_sheet(),
            "modal.close" => {
                self.layout.modals_mut().close();
            }
            "modal.close_all" => {
                self.layout.modals_mut().close_all();
            }
            "modal.reopen_last" => {
                if !self.layout.modals_mut().reopen_last() {
                    return Err("no output to reopen".into());
                }
            }
            "modal.scroll_down" | "modal.scroll_up" => {
                let sign = if action.name == "modal.scroll_down" {
                    1
                } else {
                    -1
                };
                let visible = crate::modal::max_body_rows(self.panes_height());
                if let Some(top) = self.layout.modals_mut().top_mut() {
                    top.scroll_by(sign * times as isize, visible);
                }
            }
            "pane.focus_left" | "pane.focus_right" => {
                let step = if action.name == "pane.focus_left" {
                    -1
                } else {
                    1
                };
                self.layout.focus_step(step, times);
            }
            "pane.focus_one" => self.layout.focus_one(),
            "pane.toggle" => {
                let pane: PaneId = action
                    .arg()
                    .ok_or("pane.toggle needs a pane: raw, rendered or tree")?
                    .parse()?;
                if !self.layout.toggle(pane) {
                    return Err(format!("{} is the only open pane", pane.title()));
                }
            }
            // The probe front end runs this one; it is bound only in probe mode.
            "probe.quit" => return Err("probe.quit only works in stede --probe".into()),
            other => return Err(format!("{other} has no handler yet")),
        }
        Ok(())
    }

    /// Opens the cheat sheet, or, if it is open anywhere in the stack, closes
    /// it along with anything opened on top of it.
    fn toggle_cheat_sheet(&mut self) {
        if let Some(index) = self.layout.modals().position(CHEAT_SHEET_ID) {
            self.layout.modals_mut().close_through(index);
            return;
        }
        let sheet = cheat_sheet(&self.keymap, &self.registry, self.mode);
        self.layout.modals_mut().open(sheet);
    }

    /// The view model for the screen size last passed to [`resize`](Self::resize).
    pub fn view(&self) -> View<'_> {
        let width = self.screen.0;
        let focus = self.layout.focus();
        let panes = self
            .layout
            .geometry(width)
            .into_iter()
            .map(|rect| PaneView {
                id: rect.id,
                title: rect.id.title(),
                rect,
                focused: rect.id == focus,
            })
            .collect();
        View {
            panes,
            modals: self.layout.modals().iter().collect(),
            close_key: self
                .keymap
                .keys_for(self.mode, Scope::Modal, "modal.close")
                .map(|keys| format_keys(&keys)),
            status: StatusView {
                mode: self.mode.name(),
                pending: self.resolver.pending(),
                message: self.message.clone(),
            },
        }
    }
}

/// Config problems as modal lines: each file's name, then one indented line
/// per problem (`line: key: message`). Long paths stay on their own line, so
/// what matters (the line, key and message) isn't cut off at the modal's edge.
fn problem_lines(problems: &[&crate::config::ConfigError]) -> Vec<String> {
    use crate::config::Severity;
    let mut files: Vec<&std::path::Path> = Vec::new();
    for p in problems {
        if !files.contains(&p.file.as_path()) {
            files.push(&p.file);
        }
    }
    let mut lines = Vec::new();
    for file in files {
        lines.push(file.display().to_string());
        let mut in_file: Vec<_> = problems.iter().filter(|p| p.file == file).collect();
        // Top to bottom, the order they're fixed in. Stable, so a line's
        // problems keep the order they were found in.
        in_file.sort_by_key(|p| p.line);
        for p in in_file {
            let mut line = String::from("  ");
            if p.severity == Severity::Note {
                line.push_str("note: ");
            }
            if p.line > 0 {
                line.push_str(&format!("{}: ", p.line));
            }
            if !p.key.is_empty() {
                line.push_str(&format!("{}: ", p.key));
            }
            line.push_str(&p.message);
            lines.push(line);
        }
    }
    lines
}

/// Builds the key cheat sheet for a mode from the live keymap: one section per
/// scope, each row the keys, the action and its description.
pub fn cheat_sheet(keymap: &Keymap, registry: &Registry, mode: Mode) -> Modal {
    let mut sections: Vec<Section> = Vec::new();
    for binding in keymap.bindings().filter(|b| b.mode == mode) {
        let title = match binding.scope {
            Scope::Modal => "In modals".to_owned(),
            Scope::OutputModal => "In output modals".to_owned(),
            scope => format!("{} ({scope})", capitalize(mode.name())),
        };
        let description = registry
            .get(&binding.action.name)
            .map(|s| s.description.clone())
            .unwrap_or_default();
        let row = Row {
            cells: vec![
                format_keys(&binding.keys),
                binding.action.to_string(),
                description,
            ],
        };
        match sections.iter_mut().find(|s| s.title == title) {
            Some(section) => section.rows.push(row),
            None => sections.push(Section {
                title,
                rows: vec![row],
            }),
        }
    }
    Modal::output(
        "Keys",
        ModalBody::Table {
            headers: vec!["Keys".into(), "Action".into(), "Description".into()],
            sections,
        },
    )
    .with_id(CHEAT_SHEET_ID)
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::{KeyCode, parse_keys};
    use crate::layout::PaneState;

    fn press(app: &mut App, keys: &str) -> Handled {
        let mut last = Handled::default();
        for key in parse_keys(keys, Some(app.keymap().leader())).unwrap() {
            last = app.handle_key(key);
        }
        last
    }

    #[test]
    fn a_count_typed_slowly_still_applies() {
        let mut app = App::new(AppOptions::default());
        let h = press(&mut app, "2");
        assert!(!h.wait, "a bare count asks for no timeout");
        app.handle_timeout();
        press(&mut app, "<C-l>");
        assert_eq!(app.layout().focus(), PaneId::Tree);
    }

    #[test]
    fn modal_scroll_stops_at_the_last_page() {
        let mut app = App::new(AppOptions::default());
        app.resize(80, 12);
        press(&mut app, "<F1>");
        for _ in 0..100 {
            press(&mut app, "j");
        }
        let lines = app.modals().top().unwrap().line_count();
        let visible = crate::modal::max_body_rows(11);
        assert_eq!(app.modals().top().unwrap().scroll, lines - visible);
        press(&mut app, "k");
        assert_eq!(app.modals().top().unwrap().scroll, lines - visible - 1);
    }

    #[test]
    fn notify_shows_until_the_next_action() {
        let mut app = App::new(AppOptions::default());
        app.notify("hello");
        assert_eq!(app.view().status.message.as_deref(), Some("hello"));
        press(&mut app, "<leader>e");
        assert_eq!(app.view().status.message, None);
    }

    #[test]
    fn growing_the_screen_reclamps_modal_scroll() {
        let mut app = App::new(AppOptions::default());
        app.resize(80, 16);
        press(&mut app, "<F1>");
        for _ in 0..50 {
            press(&mut app, "j");
        }
        app.resize(80, 24);
        let before = app.modals().top().unwrap().scroll;
        press(&mut app, "k");
        assert_eq!(app.modals().top().unwrap().scroll, before - 1);
        let lines = app.modals().top().unwrap().line_count();
        assert_eq!(before, lines - crate::modal::max_body_rows(23));
    }

    #[test]
    fn view_names_the_close_key() {
        let app = App::new(AppOptions::default());
        assert_eq!(app.view().close_key.as_deref(), Some("<Esc>"));
    }

    #[test]
    fn config_applies_keys_side_and_timeout() {
        use crate::config::{Config, Layer};
        let (config, errors) = Config::from_layers(&[Layer {
            path: "c.toml".into(),
            text: "[ui]\ntree_side = \"left\"\n[keys]\nleader = \",\"\ntimeout_ms = 250\n[keys.normal]\nQ = \"app.quit\"\nZZ = \"\"\n".into(),
        }]);
        assert!(errors.is_empty());
        let mut app = App::new(AppOptions {
            config,
            ..AppOptions::default()
        });
        assert!(!app.modals().is_open(), "no problems, no modal");
        assert_eq!(app.layout().order().next(), Some(PaneId::Tree));
        assert_eq!(app.key_timeout(), Duration::from_millis(250));
        press(&mut app, ",e");
        assert_eq!(app.layout().state(PaneId::Tree), PaneState::Folded);
        press(&mut app, "ZZ");
        assert!(!app.should_quit(), "ZZ was unbound");
        press(&mut app, "Q");
        assert!(app.should_quit());
    }

    #[test]
    fn config_problems_open_in_a_modal() {
        use crate::config::{Config, Layer};
        let (config, errors) = Config::from_layers(&[Layer {
            path: "c.toml".into(),
            text: "[keys.normal]\nx = \"no.such\"\nZQ = \"\"\nnn = \"\"\n[ui]\nbogus = 1\n".into(),
        }]);
        let app = App::new(AppOptions {
            config,
            config_errors: errors,
            ..AppOptions::default()
        });
        let top = app.modals().top().expect("problems modal");
        assert_eq!(top.title, "Config problems");
        let ModalBody::Text(lines) = &top.body else {
            panic!("text expected");
        };
        assert_eq!(
            lines,
            &vec![
                "c.toml".to_owned(),
                "  2: keys.normal.x: unknown action \"no.such\"".to_owned(),
                "  4: keys.normal.nn: keys \"nn\" aren't bound here, so there's nothing to unbind"
                    .to_owned(),
                "  6: ui.bogus: unknown key".to_owned(),
            ]
        );
        assert!(
            app.view()
                .status
                .message
                .unwrap()
                .starts_with("3 config error(s)")
        );
    }

    #[test]
    fn notes_alone_wait_to_be_asked_for() {
        use crate::config::{Config, Layer};
        let (config, errors) = Config::from_layers(&[Layer {
            path: "c.toml".into(),
            text: "[ui.animation]\nenabled = true\n".into(),
        }]);
        let mut app = App::new(AppOptions {
            config,
            config_errors: errors,
            ..AppOptions::default()
        });
        assert!(!app.modals().is_open(), "no modal for notes alone");
        let message = app.view().status.message.unwrap();
        assert!(
            message.starts_with("1 config setting(s) not built yet"),
            "{message}"
        );
        press(&mut app, "<leader>o");
        assert_eq!(app.modals().top().unwrap().title, "Config problems");
    }

    #[test]
    fn digit_leader_falls_back_without_crashing() {
        use crate::config::{Config, Layer};
        let (config, errors) = Config::from_layers(&[Layer {
            path: "c.toml".into(),
            text: "[keys]\nleader = \"1\"\n".into(),
        }]);
        assert_eq!(errors.len(), 1);
        let app = App::new(AppOptions {
            config,
            config_errors: errors,
            ..AppOptions::default()
        });
        assert_eq!(app.keymap().leader(), Key::char(' '));
    }

    #[test]
    fn quit() {
        let mut app = App::new(AppOptions::default());
        press(&mut app, "ZZ");
        assert!(app.should_quit());
    }

    #[test]
    fn toggle_and_focus_panes() {
        let mut app = App::new(AppOptions::default());
        press(&mut app, "<leader>e");
        assert_eq!(app.layout().state(PaneId::Tree), PaneState::Folded);
        press(&mut app, "2<C-l>");
        assert_eq!(app.layout().focus(), PaneId::Tree);
        assert_eq!(app.layout().state(PaneId::Tree), PaneState::Open);
    }

    #[test]
    fn last_pane_refuses_to_fold() {
        let mut app = App::new(AppOptions::default());
        press(&mut app, "<leader>e<leader>p<leader>r");
        assert_eq!(app.layout().state(PaneId::Raw), PaneState::Open);
        assert_eq!(
            app.view().status.message.as_deref(),
            Some("Raw is the only open pane")
        );
    }

    #[test]
    fn cheat_sheet_toggles_and_closes() {
        let mut app = App::new(AppOptions::default());
        press(&mut app, "<leader>?");
        assert_eq!(app.modals().top().unwrap().title, "Keys");
        // Pane keys don't reach the panes while a modal is open.
        press(&mut app, "<C-l>");
        assert_eq!(app.layout().focus(), PaneId::Raw);
        press(&mut app, "<leader>?");
        assert!(!app.modals().is_open());
        press(&mut app, "<F1>");
        app.handle_key(Key::plain(KeyCode::Esc));
        assert!(!app.modals().is_open());
        press(&mut app, "<leader>o");
        assert_eq!(app.modals().top().unwrap().title, "Keys");
    }

    #[test]
    fn cheat_sheet_hotkey_closes_it_from_under_another_modal() {
        let mut app = App::new(AppOptions::default());
        press(&mut app, "<F1>");
        app.layout
            .modals_mut()
            .open(Modal::output("Other", ModalBody::Text(vec![])));
        press(&mut app, "<F1>");
        assert!(
            !app.modals().is_open(),
            "closed through the sheet, no second copy"
        );
        // Another output that happens to be titled "Keys" isn't mistaken for it.
        app.layout
            .modals_mut()
            .open(Modal::output("Keys", ModalBody::Text(vec![])));
        press(&mut app, "<F1>");
        assert_eq!(app.modals().len(), 2);
    }

    #[test]
    fn cheat_sheet_lists_every_binding_with_a_description() {
        let app = App::new(AppOptions::default());
        let sheet = cheat_sheet(app.keymap(), app.registry(), Mode::Normal);
        let ModalBody::Table { sections, .. } = sheet.body else {
            panic!("table expected");
        };
        let rows: Vec<&Row> = sections.iter().flat_map(|s| &s.rows).collect();
        assert_eq!(
            rows.len(),
            app.keymap()
                .bindings()
                .filter(|b| b.mode == Mode::Normal)
                .count()
        );
        assert!(
            rows.iter()
                .all(|r| r.cells.len() == 3 && !r.cells[2].is_empty())
        );
        assert!(
            rows.iter()
                .any(|r| r.cells[0] == "<Space>e" && r.cells[1] == "pane.toggle tree")
        );
    }

    #[test]
    fn pending_keys_show_and_time_out() {
        let mut app = App::new(AppOptions::default());
        let h = press(&mut app, "<C-w>");
        assert!(h.wait);
        assert_eq!(app.view().status.pending, "<C-w>");
        app.handle_timeout();
        assert_eq!(app.view().status.pending, "");
        assert_eq!(
            app.view().status.message.as_deref(),
            Some("<C-w> is not bound")
        );
    }

    #[test]
    fn reopen_with_nothing_reports() {
        let mut app = App::new(AppOptions::default());
        press(&mut app, "<leader>o");
        assert_eq!(
            app.view().status.message.as_deref(),
            Some("no output to reopen")
        );
    }

    #[test]
    fn bad_pane_argument_reports() {
        let mut app = App::new(AppOptions::default());
        let err = app.run(&Action::parse("pane.toggle files").unwrap(), None);
        assert!(err.unwrap_err().contains("unknown pane"));
        let err = app.run(&Action::new("pane.toggle"), None);
        assert!(err.unwrap_err().contains("needs a pane"));
    }

    #[test]
    fn view_marks_focus() {
        let app = App::new(AppOptions::default());
        let view = app.view();
        assert_eq!(view.panes.len(), 3);
        assert!(view.panes.iter().filter(|p| p.focused).count() == 1);
        assert_eq!(view.status.mode, "normal");
    }
}
