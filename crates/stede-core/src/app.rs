// SPDX-License-Identifier: GPL-3.0-or-later
//! The editor loop: key events in, actions run, a view model out.
//!
//! A front end owns the event loop and the clock. It sends each key press to
//! [`App::handle_key`]; if that reports a pending sequence, it calls
//! [`App::handle_timeout`] once [`App::key_timeout`] passes with no new key.
//! Then it draws [`App::view`].

use std::path::PathBuf;
use std::time::Duration;

use crate::action::{Action, Registry};
use crate::doc::Doc;
use crate::io::{Request, ServiceEvent};
use crate::key::{Key, format_keys};
use crate::keymap::{Context, Keymap, Mode, Resolver, Scope, Step};
use crate::layout::{Layout, LayoutOptions, PaneId};
use crate::modal::{Modal, ModalBody, ModalStack, Row, Section};
use crate::search::{Compiled, Direction, MAX_PATTERN, Search};
use crate::sync::Rendered;
use crate::tree::Tree;
use crate::view::{
    PaneView, RawView, RenderedView, STATUS_ROWS, SearchView, StatusView, TreeView, View,
};

/// Identifies the key cheat sheet among open modals, so its hotkey toggles it.
const CHEAT_SHEET_ID: &str = "help.keys";

/// Identifies the config problems modal, so a reload replaces it.
const CONFIG_PROBLEMS_ID: &str = "config.problems";

/// The character a key types into a prompt, if it types one.
fn printable(key: Key) -> Option<char> {
    match key.code() {
        crate::key::KeyCode::Char(c) if !key.mods().ctrl && !key.mods().alt && !c.is_control() => {
            Some(c)
        }
        _ => None,
    }
}

/// Blocks longer than this many source lines sync by line share instead of
/// wrapped rows, so a key never walks thousands of lines.
const ROW_SHARE_LINES: usize = 512;

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
    /// The folder the file tree shows. `None` shows no tree content.
    pub root: Option<PathBuf>,
    /// The file opened on the command line, if one was: it's selected in the
    /// tree once that shows.
    pub file: Option<PathBuf>,
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
    /// The config problems found at start, kept so `config.problems` can
    /// show them any time (other output modals can't push them out).
    config_problems: Option<Modal>,
    tree: Tree,
    /// Work for the services, collected until the front end takes it.
    outbox: Vec<Request>,
    /// The file last opened from the tree (or on the command line).
    current_file: Option<PathBuf>,
    /// The open document, once read.
    doc: Option<Doc>,
    /// Why the current file couldn't be opened, if it couldn't.
    doc_error: Option<String>,
    /// Loads so far, to tell front ends when the document changed.
    loads: u64,
    /// The Markdown flavor from config, passed with each open.
    flavor: Option<String>,
    line_numbers: bool,
    /// The rendered pane's layout and place.
    rendered: Rendered,
    /// The search in force (its matches are lit), if any.
    search: Option<Search>,
    /// The pattern being typed, and the search in force before it began
    /// (kept for Esc).
    prompt: Option<(Direction, String, Option<Search>)>,
    /// Where the last search jump left the raw pane: `n` steps on from the
    /// current match only while the view is still there.
    search_at: Option<(usize, usize)>,
    /// Whether the search's matches are lit (`search.off` hides them but
    /// keeps the search for `n`, like Vim's `:noh`).
    search_lit: bool,
    /// The line-numbers setting config last gave, so a reload that didn't
    /// change it keeps the session's `<leader>n` choice.
    config_line_numbers: Option<bool>,
}

impl App {
    pub fn new(options: AppOptions) -> Self {
        let registry = Registry::with_builtins();
        let layout = Layout::new(LayoutOptions {
            tree_side: options.config.tree_side,
            ..options.layout
        });
        let mut app = Self {
            keymap: Keymap::defaults(&registry),
            registry,
            resolver: Resolver::new(),
            layout,
            mode: Mode::Normal,
            message: None,
            key_timeout: options.config.key_timeout,
            quit: false,
            screen: (80, 24),
            config_problems: None,
            tree: Tree::default(),
            outbox: Vec::new(),
            // Set after config is applied, so start-up asks for it once.
            current_file: None,
            doc: None,
            doc_error: None,
            loads: 0,
            flavor: None,
            line_numbers: true,
            config_line_numbers: None,
            rendered: Rendered::default(),
            search: None,
            prompt: None,
            search_at: None,
            search_lit: true,
        };
        if let Some(root) = options.root {
            let (tree, first) = Tree::new(root, options.file.clone());
            app.tree = tree;
            app.outbox.push(Request::ListDir(first));
        }
        app.apply_config(&options.config, &options.config_errors);
        if let Some(file) = options.file {
            app.current_file = Some(file.clone());
            app.request_open(file);
        }
        app
    }

    fn request_open(&mut self, path: PathBuf) {
        self.outbox.push(Request::OpenFile {
            path,
            flavor: self.flavor.clone(),
        });
    }

    /// Rows the raw pane shows: the pane less its borders.
    fn raw_height(&self) -> usize {
        usize::from(self.panes_height().saturating_sub(2))
    }

    /// The raw pane's number column and text width, in cells. The front end
    /// draws with these, so its rows break where scrolling counts them.
    ///
    /// While the raw pane is folded or off, the widths are those it will have
    /// when it opens, so positions set meanwhile still fit it.
    fn raw_columns(&self) -> (usize, usize) {
        // Only a folded raw pane needs a copy, opened, to measure.
        let opened;
        let layout = if self.layout.state(PaneId::Raw) == crate::layout::PaneState::Open {
            &self.layout
        } else {
            let mut copy = self.layout.clone();
            copy.focus_pane(PaneId::Raw);
            opened = copy;
            &opened
        };
        let inner = layout
            .geometry(self.screen.0)
            .into_iter()
            .find(|r| r.id == PaneId::Raw && !r.folded)
            .map_or(0, |r| usize::from(r.width.saturating_sub(2)));
        let total = self.doc.as_ref().map_or(1, Doc::line_count);
        let gutter = if self.line_numbers {
            total.max(1).to_string().len() + 1
        } else {
            0
        };
        // A pane narrower than the gutter drops the numbers.
        let gutter = if inner > gutter + 1 { gutter } else { 0 };
        (gutter, inner.saturating_sub(gutter).max(1))
    }

    /// Applies a config: keys, timeout and tree side. Problems open in a
    /// modal if any is an error (config was skipped); notes only get a hint.
    /// Used at start and when a config file changes on disk.
    pub fn apply_config(
        &mut self,
        config: &crate::config::Config,
        config_errors: &[crate::config::ConfigError],
    ) {
        let (keymap, key_errors) = Keymap::from_config(&self.registry, config);
        self.keymap = keymap;
        self.resolver.reset();
        self.key_timeout = config.key_timeout;
        self.layout.set_tree_side(config.tree_side);
        if self.config_line_numbers != Some(config.line_numbers) {
            self.line_numbers = config.line_numbers;
            self.config_line_numbers = Some(config.line_numbers);
        }
        let flavor = config.flavor.as_ref().map(|(name, _)| name.clone());
        let flavor_changed = flavor != self.flavor;
        self.flavor = flavor;
        // A new flavor parses the open file differently: read it again.
        // Also while it's still loading: that read used the old flavor (the
        // services fold the new request into it).
        if flavor_changed && let Some(file) = self.current_file.clone() {
            self.request_open(file);
        }
        self.config_problems = None;
        // A reload replaces the problems shown, or clears them once fixed.
        self.layout.modals_mut().remove(CONFIG_PROBLEMS_ID);
        let app = self;
        let all: Vec<&crate::config::ConfigError> =
            config_errors.iter().chain(&key_errors).collect();
        if all.is_empty() {
            // A clean reload clears the "config error(s)" hint.
            if app.config_problems.is_none() {
                app.message = None;
            }
        } else {
            use crate::config::Severity;
            let errors = all.iter().filter(|e| e.severity == Severity::Error).count();
            let notes = all.len() - errors;
            let show = app
                .keymap
                .keys_for(Mode::Normal, Scope::Global, "config.problems")
                .map(|k| format_keys(&k))
                .unwrap_or_else(|| "config.problems".into());
            let modal = Modal::output("Config problems", ModalBody::Text(problem_lines(&all)))
                .with_id(CONFIG_PROBLEMS_ID);
            if errors > 0 {
                // Errors mean config was skipped: show them at once.
                app.notify(format!(
                    "{errors} config error(s): {show} to see them again"
                ));
                app.layout.modals_mut().open(modal.clone());
            } else {
                // Notes only (settings not built yet): don't get in the way.
                app.notify(format!(
                    "{notes} config setting(s) not built yet: {show} to see them"
                ));
            }
            app.config_problems = Some(modal);
        }
    }

    /// Work for the services since the last call: hand it to `stede-services`.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.outbox)
    }

    /// A result from the services.
    pub fn handle_event(&mut self, event: ServiceEvent) -> Handled {
        match event {
            ServiceEvent::Listed { dir, result } => {
                let forgotten = self.tree.listed(&dir, result);
                let known = forgotten.is_some();
                for gone in forgotten.into_iter().flatten() {
                    self.outbox.push(Request::Unwatch(gone));
                }
                self.rescroll_tree();
                Handled {
                    redraw: known,
                    wait: false,
                }
            }
            ServiceEvent::Changed(paths) => {
                for dir in self.tree.stale(&paths) {
                    self.outbox.push(Request::ListDir(dir));
                }
                // The open file changed on disk: read it again.
                if let Some(file) = self.current_file.clone()
                    && paths.contains(&file)
                {
                    self.request_open(file);
                }
                Handled::default()
            }
            ServiceEvent::Opened { path, result } => {
                // Only the file asked for last counts; an older open that
                // finishes late is dropped.
                if self.current_file.as_ref() != Some(&path) {
                    return Handled::default();
                }
                match result {
                    Ok(document) => {
                        self.loads += 1;
                        // A reload keeps the reader's place.
                        let place = self
                            .doc
                            .as_ref()
                            .filter(|d| d.path == path)
                            .map_or((0, 0), Doc::position);
                        self.doc = Some(Doc::new(document, self.loads));
                        // Matches move with the text.
                        if let Some(doc) = self.doc.as_ref() {
                            let before = self.prompt.as_mut().and_then(|p| p.2.as_mut());
                            for search in self.search.iter_mut().chain(before) {
                                search.rescan(Self::doc_lines(doc));
                            }
                        }
                        self.search_at = None;
                        let (_, width) = self.raw_columns();
                        let height = self.raw_height();
                        if let Some(doc) = self.doc.as_mut() {
                            doc.set_position(place.0, place.1, width, height);
                        }
                        self.doc_error = None;
                    }
                    Err(e) => {
                        let name = path
                            .file_name()
                            .unwrap_or(path.as_os_str())
                            .to_string_lossy();
                        if self.doc.as_ref().is_some_and(|d| d.path == path) {
                            // A failed reload keeps what was shown.
                            self.message = Some(format!("couldn't reload {name}: {e}"));
                        } else {
                            self.doc = None;
                            self.doc_error = Some(format!("can't open {name}: {e}"));
                        }
                    }
                }
                Handled {
                    redraw: true,
                    wait: false,
                }
            }
            // Re-reading config files is the front end's job (it found them).
            ServiceEvent::ConfigChanged { .. } => Handled::default(),
        }
    }

    /// The file last opened, if any.
    pub fn current_file(&self) -> Option<&std::path::Path> {
        self.current_file.as_deref()
    }

    /// Rows the tree pane shows: the pane less its borders.
    fn tree_height(&self) -> usize {
        usize::from(self.panes_height().saturating_sub(2))
    }

    fn rescroll_tree(&mut self) {
        let height = self.tree_height();
        self.tree.scroll_for(height);
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
        self.rescroll_tree();
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
        self.leave_covered_search();
        let mut keys = self.resolver.pending_keys().to_vec();
        keys.push(key);
        let ctx = self.context();
        let step = self.resolver.feed(&self.keymap, ctx, key);
        self.type_or_apply(step, &keys)
    }

    /// A modal opened over the search prompt (a config reload's problems):
    /// drop the pattern, so the modal's keys work.
    fn leave_covered_search(&mut self) {
        if self.mode == Mode::Search && !self.layout.modals().is_empty() {
            if let Some((_, _, before)) = self.prompt.take() {
                self.search = before;
            }
            self.mode = Mode::Normal;
            self.resolver.reset();
        }
    }

    /// Typing a pattern: keys that ended up bound to nothing (a lone letter,
    /// or the start of a longer binding that didn't follow) type themselves.
    ///
    /// With several keys (`j` of a `jk` binding, then `<CR>`), the earlier
    /// ones type and the last is fed again alone, so it can run its own
    /// binding (Enter confirms, Esc cancels).
    fn type_or_apply(&mut self, step: Step, keys: &[Key]) -> Handled {
        if self.mode == Mode::Search
            && matches!(step, Step::Unbound(_) | Step::Cancelled)
            && let [prefix @ .., last] = keys
            && !prefix.is_empty()
        {
            let typed: String = prefix.iter().filter_map(|k| printable(*k)).collect();
            if !typed.is_empty() {
                self.apply(Step::Run {
                    action: Action {
                        name: "search.insert".into(),
                        args: vec![typed],
                    },
                    count: None,
                });
            }
            let ctx = self.context();
            let step = self.resolver.feed(&self.keymap, ctx, *last);
            return self.type_or_apply(step, &[*last]);
        }
        if self.mode == Mode::Search && matches!(step, Step::Unbound(_)) {
            let typed: String = keys.iter().filter_map(|k| printable(*k)).collect();
            if !typed.is_empty() {
                let insert = Action {
                    name: "search.insert".into(),
                    args: vec![typed],
                };
                return self.apply(Step::Run {
                    action: insert,
                    count: None,
                });
            }
        }
        self.apply(step)
    }

    /// The document's lines, for searching.
    fn doc_lines(doc: &Doc) -> impl Iterator<Item = &str> + '_ {
        (0..doc.line_count()).filter_map(|i| doc.line(i))
    }

    /// Re-runs the pattern being typed, lighting its matches as it grows.
    ///
    /// Each key rescans, so it scans a budget of bytes from the view's line
    /// on (then from the top); Enter scans the rest.
    fn search_preview(&mut self) {
        if let (Some((direction, text, _)), Some(doc)) = (&self.prompt, &self.doc) {
            let top = doc.position().0;
            let n = doc.line_count();
            let lines = (top..n)
                .chain(0..top)
                .filter_map(|i| doc.line(i).map(|l| (i, l)));
            self.search = Some(Search::preview(text, *direction, lines));
            self.search_lit = true;
        }
    }

    /// Goes to the next match from the raw pane's top line, puts it at the
    /// top of the raw pane, and brings the rendered pane along.
    /// `times` steps (a count on `n`) move through the matches, then the
    /// panes are placed once.
    fn search_step(
        &mut self,
        direction: Direction,
        inclusive: bool,
        times: u32,
    ) -> Result<(), String> {
        let Some(doc) = self.doc.as_ref() else {
            return Err("no file open".into());
        };
        let here = doc.position();
        let Some(search) = self.search.as_mut() else {
            return Err("no search yet: / to start one".into());
        };
        // Scrolled away since the last jump: search from the view, not from
        // the old match (forward, a match on the top line counts).
        let moved = self.search_at != Some(here);
        if moved {
            search.clear_current();
        }
        let inclusive = inclusive || (moved && direction == Direction::Forward);
        let pattern = search.pattern.clone();
        if search.compiled == Compiled::TooBig {
            return Err("search pattern too big to compile".into());
        }
        let Some((mut m, mut wrapped)) = search.step(here.0, direction, inclusive) else {
            return Err(format!("pattern not found: {pattern}"));
        };
        for _ in 1..times {
            if let Some((next, w)) = search.step(m.line, direction, false) {
                m = next;
                wrapped |= w;
            }
        }
        self.search_lit = true;
        let (_, width) = self.raw_columns();
        let height = self.raw_height();
        if let Some(doc) = self.doc.as_mut() {
            // The match's wrapped row, so a match deep in a long paragraph line
            // is on screen.
            let row = doc
                .line(m.line)
                .map_or(0, |l| crate::text::row_at(l, width, m.start));
            doc.set_position(m.line, row, width, height);
            self.search_at = Some(doc.position());
        }
        self.rendered_follow_raw();
        // Near the end the raw pane stops at its last page: the rendered
        // pane goes to its end too, where the match is.
        self.sync_ends();
        if wrapped {
            self.message = Some(match direction {
                Direction::Forward => "search hit the end, continuing at the top".into(),
                Direction::Backward => "search hit the top, continuing at the end".into(),
            });
        }
        Ok(())
    }

    /// Call when [`key_timeout`](Self::key_timeout) passes after a pending key.
    pub fn handle_timeout(&mut self) -> Handled {
        self.leave_covered_search();
        let keys = self.resolver.pending_keys().to_vec();
        let ctx = self.context();
        match self.resolver.timeout(&self.keymap, ctx) {
            Some(step) => self.type_or_apply(step, &keys),
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
            "config.problems" => match &self.config_problems {
                Some(modal) => {
                    let modal = modal.clone();
                    self.layout.modals_mut().open(modal);
                }
                None => return Err("no config problems".into()),
            },
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
            "tree.down" | "tree.up" => {
                let sign: isize = if action.name == "tree.down" { 1 } else { -1 };
                self.tree.move_by(sign * times as isize);
                self.rescroll_tree();
            }
            "tree.top" | "tree.bottom" => {
                self.tree.move_to_end(action.name == "tree.bottom");
                self.rescroll_tree();
            }
            "tree.open" => self.tree_open()?,
            "view.down" | "view.up" | "view.half_down" | "view.half_up" | "view.page_down"
            | "view.page_up" | "view.top" | "view.bottom" => {
                // The focused reader pane scrolls; the other one follows.
                if self.layout.focus() == PaneId::Rendered {
                    // A counted G already put the raw pane on its line.
                    if !self.scroll_rendered(&action.name, times, count) {
                        self.raw_follow_rendered();
                    }
                } else {
                    self.scroll_raw(&action.name, times, count);
                    self.rendered_follow_raw();
                }
                self.sync_ends();
            }
            "search.forward" | "search.backward" => {
                let direction = if action.name == "search.forward" {
                    Direction::Forward
                } else {
                    Direction::Backward
                };
                self.prompt = Some((direction, String::new(), self.search.take()));
                self.mode = Mode::Search;
            }
            "search.insert" => {
                // Words of a bound `search.insert a b` join with spaces.
                let typed = action.args.join(" ");
                if let (Some((_, text, _)), Some(c)) = (
                    self.prompt.as_mut(),
                    Some(typed.as_str()).filter(|t| !t.is_empty()),
                ) {
                    let room = MAX_PATTERN.saturating_sub(text.chars().count());
                    text.extend(c.chars().take(room));
                    if c.chars().count() > room {
                        self.message =
                            Some(format!("search patterns stop at {MAX_PATTERN} characters"));
                    }
                }
                self.search_preview();
            }
            "search.backspace" | "search.clear" => {
                if let Some((_, text, _)) = self.prompt.as_mut() {
                    if action.name == "search.clear" {
                        text.clear();
                    } else {
                        text.pop();
                    }
                }
                self.search_preview();
            }
            "search.cancel" => {
                // Back to the search there was before, as if never typed.
                if let Some((_, _, before)) = self.prompt.take() {
                    self.search = before;
                }
                self.mode = Mode::Normal;
            }
            "search.confirm" => {
                let prompt = self.prompt.take();
                self.mode = Mode::Normal;
                match prompt {
                    // An empty pattern repeats the last search, as in Vim.
                    Some((direction, text, before)) if text.is_empty() => {
                        self.search = before;
                        if let Some(s) = self.search.as_mut() {
                            s.direction = direction;
                        }
                        self.search_step(direction, false, 1)?;
                    }
                    Some((direction, _, _)) => {
                        // The preview scanned only part of a big file.
                        if let (Some(search), Some(doc)) = (self.search.as_mut(), self.doc.as_ref())
                            && search.partial
                        {
                            search.rescan(Self::doc_lines(doc));
                        }
                        let literal = self
                            .search
                            .as_ref()
                            .is_some_and(|s| s.compiled == Compiled::Literal);
                        // A new pattern finds from here: a match on the top line counts.
                        self.search_step(direction, true, 1)?;
                        if literal && self.message.is_none() {
                            self.message = Some("not a valid regex: searched as plain text".into());
                        }
                    }
                    None => {}
                }
            }
            "search.next" | "search.prev" => {
                let Some(forward) = self
                    .search
                    .as_ref()
                    .map(|s| s.direction == Direction::Forward)
                else {
                    return Err("no search yet: / to start one".into());
                };
                // `N` goes against the search's own direction.
                let ahead = (action.name == "search.next") == forward;
                let direction = if ahead {
                    Direction::Forward
                } else {
                    Direction::Backward
                };
                self.search_step(direction, false, times)?;
            }
            "search.off" => self.search_lit = false,
            "rendered.jump" => {
                // Show the top block's source at the top of the raw pane.
                let line = self
                    .doc
                    .as_ref()
                    .and_then(|d| d.model.line_of_block(self.rendered.top_block()))
                    .map(|l| l.saturating_sub(1));
                if let Some(line) = line {
                    // Open the raw pane first: its width sets the position.
                    self.layout.focus_pane(PaneId::Raw);
                    let (_, width) = self.raw_columns();
                    let height = self.raw_height();
                    if let Some(doc) = self.doc.as_mut() {
                        doc.set_position(line, 0, width, height);
                    }
                }
            }
            "line_numbers.toggle" => self.line_numbers = !self.line_numbers,
            "tree.close" => {
                let rows = self.tree.rows();
                if let Some(row) = self.tree.selected_index(&rows).map(|i| &rows[i]) {
                    if row.is_dir && row.expanded {
                        self.tree.collapse(&row.path.clone());
                    } else {
                        self.tree.select_parent();
                    }
                }
                self.rescroll_tree();
            }
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

    /// The front end's layout of the rendered pane: each block's height in
    /// rows, for document `generation` at `width` cells. Call it when either
    /// changes (before drawing). A new document puts the rendered pane where
    /// the raw pane is.
    pub fn set_rendered_layout(&mut self, generation: u64, width: usize, heights: Vec<usize>) {
        let new_doc = self.rendered.generation != generation;
        let resized = self.rendered.width != width;
        let height = self.raw_height();
        self.rendered.set_layout(generation, width, heights, height);
        // A new document starts where the raw pane is; a new width (a resize,
        // a pane folded or opened) re-syncs from whichever pane has focus.
        if new_doc || (resized && self.layout.focus() != PaneId::Rendered) {
            self.rendered_follow_raw();
        } else if resized {
            self.raw_follow_rendered();
        }
        self.sync_ends();
    }

    /// At the end of one reader pane, the other goes to its end too: the
    /// rendered pane can be taller (table borders, rules) than its source.
    fn sync_ends(&mut self) {
        let height = self.raw_height();
        let (_, width) = self.raw_columns();
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        // Only a pane that can scroll can be "at its end": a note that fits
        // on screen sits at the top, which is also its last page.
        let raw_max = doc.max_position(width, height);
        let raw_end = raw_max != (0, 0) && doc.position() == raw_max;
        if self.layout.focus() == PaneId::Rendered {
            if self.rendered.can_scroll(height) && self.rendered.at_end(height) {
                doc.set_position(raw_max.0, raw_max.1, width, height);
            }
        } else if raw_end {
            self.rendered.to_end(height);
        }
    }

    /// Whether the rendered layout needs reporting for this document and width.
    pub fn rendered_layout_is_current(&self, generation: u64, width: usize) -> bool {
        self.rendered.is_current(generation, width)
    }

    /// Puts the rendered pane where the raw pane is: the block for the raw
    /// pane's top line, at the same share of the block's raw rows.
    fn rendered_follow_raw(&mut self) {
        let height = self.raw_height();
        let (_, width) = self.raw_columns();
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let (top, row) = doc.position();
        let line = top + 1;
        let Some(block) = doc.model.block_at_line(line) else {
            self.rendered.set_position(0, 0, height);
            return;
        };
        let source = doc.model.blocks[block].source;
        if source.contains(line) {
            let first = source.start - 1;
            let lines = source.end - first;
            if lines > ROW_SHARE_LINES {
                // A huge block (a log read as one code block): share by
                // lines, which costs the same however long it is.
                self.rendered.follow(block, top - first, lines, height);
            } else {
                let into = (first..top).map(|i| doc.rows(i, width)).sum::<usize>() + row;
                let total = (first..source.end)
                    .map(|i| doc.rows(i, width))
                    .sum::<usize>();
                self.rendered.follow(block, into, total, height);
            }
        } else {
            // Between blocks: the next block's top.
            self.rendered.follow(block, 0, 1, height);
        }
    }

    /// Puts the raw pane where the rendered pane is: the top block's source,
    /// at the same share of its raw rows as is scrolled past in the block.
    fn raw_follow_rendered(&mut self) {
        let height = self.raw_height();
        let (_, width) = self.raw_columns();
        let (block, row, rows) = self.rendered.share();
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let Some(source) = doc.model.blocks.get(block).map(|b| b.source) else {
            return;
        };
        let (first, last) = (source.start - 1, source.end - 1);
        if last - first + 1 > ROW_SHARE_LINES {
            // A huge block: share by lines (see `rendered_follow_raw`).
            let line = first + row * (last - first + 1) / rows.max(1);
            doc.set_position(line, 0, width, height);
            return;
        }
        let total: usize = (first..=last).map(|i| doc.rows(i, width)).sum();
        let target = row * total / rows.max(1);
        let (mut line, mut before) = (first, 0);
        while line < last && before + doc.rows(line, width) <= target {
            before += doc.rows(line, width);
            line += 1;
        }
        doc.set_position(line, target - before, width, height);
    }

    /// Scrolls the rendered pane by rows, half pages or pages, or to an end.
    /// Returns whether it placed the raw pane itself (a counted `gg`/`G`).
    fn scroll_rendered(&mut self, name: &str, times: u32, count: Option<u32>) -> bool {
        let height = self.raw_height().max(1);
        let n = times as isize;
        let half = (height / 2).max(1) as isize;
        match name {
            "view.down" => self.rendered.scroll_rows(n, height),
            "view.up" => self.rendered.scroll_rows(-n, height),
            "view.half_down" => self.rendered.scroll_rows(n * half, height),
            "view.half_up" => self.rendered.scroll_rows(-n * half, height),
            "view.page_down" => self.rendered.scroll_rows(n * height as isize, height),
            "view.page_up" => self.rendered.scroll_rows(-n * height as isize, height),
            // With a count, go to that source line's block, as `50G` does.
            "view.top" | "view.bottom" if count.is_some() => {
                // Put the raw pane on that line, and the rendered pane on it.
                let line = (count.unwrap_or(1) as usize).saturating_sub(1);
                let (_, width) = self.raw_columns();
                if let Some(doc) = self.doc.as_mut() {
                    doc.set_position(line, 0, width, height);
                }
                self.rendered_follow_raw();
                return true;
            }
            "view.top" => self.rendered.set_position(0, 0, height),
            _ => self.rendered.to_end(height),
        }
        false
    }

    /// Scrolls the raw pane by lines, half pages or pages, or to an end.
    /// With a count, `gg` and `G` go to that line, as in Vim (`50G`).
    fn scroll_raw(&mut self, name: &str, times: u32, count: Option<u32>) {
        let height = self.raw_height().max(1);
        let (_, width) = self.raw_columns();
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let n = times as isize;
        let half = (height / 2).max(1) as isize;
        // Lines, half pages and pages move by wrapped rows.
        match name {
            "view.down" => doc.scroll_rows(n, width, height),
            "view.up" => doc.scroll_rows(-n, width, height),
            "view.half_down" => doc.scroll_rows(n * half, width, height),
            "view.half_up" => doc.scroll_rows(-n * half, width, height),
            "view.page_down" => doc.scroll_rows(n * height as isize, width, height),
            "view.page_up" => doc.scroll_rows(-n * height as isize, width, height),
            "view.top" | "view.bottom" if count.is_some() => {
                let line = (count.unwrap_or(1) as usize).saturating_sub(1);
                doc.set_position(line, 0, width, height);
            }
            "view.top" => doc.set_position(0, 0, width, height),
            _ => {
                let (line, row) = doc.max_position(width, height);
                doc.set_position(line, row, width, height);
            }
        }
    }

    /// Opens the selected tree entry: a folder opens or closes, a file is
    /// handed to the services to load.
    fn tree_open(&mut self) -> Result<(), String> {
        let rows = self.tree.rows();
        let Some(row) = self.tree.selected_index(&rows).map(|i| rows[i].clone()) else {
            return Err("the tree is empty".into());
        };
        if row.is_dir {
            if row.expanded {
                self.tree.collapse(&row.path);
            } else if let Some(dir) = self.tree.expand(&row.path) {
                self.outbox.push(Request::ListDir(dir));
            }
        } else {
            let showing = self.doc.as_ref().is_some_and(|d| d.path == row.path);
            // Open it unless it's already showing; after a failed open,
            // Enter tries again.
            if !showing || self.doc_error.is_some() {
                if !showing {
                    // Don't leave the last file up under the new one's name.
                    self.doc = None;
                }
                self.current_file = Some(row.path.clone());
                self.doc_error = None;
                self.request_open(row.path.clone());
            }
        }
        self.rescroll_tree();
        Ok(())
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
        let rows = self.tree.rows();
        let tree = TreeView {
            root: self.tree.root().map(|r| {
                r.file_name()
                    .unwrap_or(r.as_os_str())
                    .to_string_lossy()
                    .into_owned()
            }),
            selected: self.tree.selected_index(&rows),
            scroll: self.tree.scroll(),
            loading: self.tree.is_loading(),
            error: self.tree.root_error().map(str::to_owned),
            rows,
        };
        let raw = match &self.doc {
            Some(doc) => {
                let (top, first_row) = doc.position();
                let (gutter, text_width) = self.raw_columns();
                RawView {
                    title: doc
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned()),
                    lines: (top..doc.line_count())
                        .take(self.raw_height())
                        .filter_map(|i| doc.line(i).map(|l| (i + 1, l, doc.line_kind(i))))
                        .collect(),
                    first_row,
                    gutter,
                    text_width,
                    total: doc.line_count(),
                    line_numbers: self.line_numbers,
                    message: None,
                    generation: doc.generation,
                }
            }
            None => RawView {
                title: None,
                lines: Vec::new(),
                first_row: 0,
                gutter: 0,
                text_width: 0,
                total: 0,
                line_numbers: self.line_numbers,
                message: Some(match (&self.doc_error, &self.current_file) {
                    (Some(e), _) => e.clone(),
                    (None, Some(_)) => "Loading…".to_owned(),
                    (None, None) => "No file open: pick one in the file tree".to_owned(),
                }),
                generation: 0,
            },
        };
        let rendered = RenderedView {
            model: self.doc.as_ref().map(|d| &d.model),
            generation: self.doc.as_ref().map_or(0, |d| d.generation),
            position: self.rendered.position(),
            laid_out: self
                .doc
                .as_ref()
                .is_some_and(|d| self.rendered.generation == d.generation),
        };
        View {
            panes,
            tree,
            raw,
            rendered,
            modals: self.layout.modals().iter().collect(),
            close_key: self
                .keymap
                .keys_for(self.mode, Scope::Modal, "modal.close")
                .map(|keys| format_keys(&keys)),
            search: self
                .search
                .as_ref()
                .filter(|_| self.search_lit)
                .map(|s| SearchView {
                    capped: s.at_least(),
                    regex: s.regex(),
                    current_line: s.current().map(|m| m.line + 1),
                    position: s.position(),
                }),
            status: StatusView {
                prompt: self.prompt.as_ref().map(|(d, text, _)| {
                    let sign = if *d == Direction::Forward { '/' } else { '?' };
                    format!("{sign}{text}")
                }),
                mode: self.mode.name(),
                pending: self.resolver.pending(),
                message: self.message.clone(),
                // Relative to the tree's folder when it's inside it: shorter.
                file: self.current_file.as_ref().map(|p| {
                    self.tree
                        .root()
                        .and_then(|r| p.strip_prefix(r).ok())
                        .unwrap_or(p)
                        .display()
                        .to_string()
                }),
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
        // Enough to reach the end, however long the sheet grows.
        for _ in 0..500 {
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
        // Enough to reach the end, however long the sheet grows.
        for _ in 0..500 {
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
        // Another output modal can't push the problems out of reach.
        press(&mut app, "<F1><Esc><leader>c");
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

    fn entry(path: &str, is_dir: bool) -> crate::io::Entry {
        let path = PathBuf::from(path);
        crate::io::Entry {
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            path,
            is_dir,
        }
    }

    fn tree_app() -> App {
        let mut app = App::new(AppOptions {
            root: Some("/p".into()),
            ..AppOptions::default()
        });
        assert_eq!(app.take_requests(), vec![Request::ListDir("/p".into())]);
        app.handle_event(ServiceEvent::Listed {
            dir: "/p".into(),
            result: Ok(vec![entry("/p/docs", true), entry("/p/a.md", false)]),
        });
        press(&mut app, "2<C-l>");
        assert_eq!(app.layout().focus(), PaneId::Tree);
        app
    }

    #[test]
    fn tree_keys_move_open_and_close() {
        let mut app = tree_app();
        assert_eq!(app.view().tree.selected, Some(0));
        press(&mut app, "l");
        assert_eq!(
            app.take_requests(),
            vec![Request::ListDir("/p/docs".into())]
        );
        app.handle_event(ServiceEvent::Listed {
            dir: "/p/docs".into(),
            result: Ok(vec![entry("/p/docs/x.md", false)]),
        });
        press(&mut app, "j");
        assert_eq!(app.view().tree.rows[1].name, "x.md");
        assert_eq!(app.view().tree.selected, Some(1));
        press(&mut app, "<CR>");
        assert_eq!(
            app.take_requests(),
            vec![Request::OpenFile {
                path: "/p/docs/x.md".into(),
                flavor: None
            }]
        );
        assert_eq!(
            app.current_file(),
            Some(std::path::Path::new("/p/docs/x.md"))
        );
        press(&mut app, "h");
        assert_eq!(
            app.view().tree.selected,
            Some(0),
            "h on a file goes to its folder"
        );
        press(&mut app, "h");
        assert_eq!(
            app.view().tree.rows.len(),
            2,
            "h on an open folder closes it"
        );
        press(&mut app, "G");
        assert_eq!(app.view().tree.selected, Some(1));
        press(&mut app, "gg");
        assert_eq!(app.view().tree.selected, Some(0));
    }

    #[test]
    fn tree_keys_only_apply_in_the_tree() {
        let mut app = tree_app();
        press(&mut app, "<C-h><C-h>");
        assert_eq!(app.layout().focus(), PaneId::Raw);
        press(&mut app, "G");
        assert_eq!(app.view().tree.selected, Some(0));
    }

    #[test]
    fn changes_on_disk_ask_for_fresh_listings() {
        let mut app = tree_app();
        app.handle_event(ServiceEvent::Changed(vec!["/p/new.md".into()]));
        assert_eq!(app.take_requests(), vec![Request::ListDir("/p".into())]);
        // A listing for a folder the tree never asked for changes nothing.
        let h = app.handle_event(ServiceEvent::Listed {
            dir: "/elsewhere".into(),
            result: Ok(vec![]),
        });
        assert!(!h.redraw);
    }

    #[test]
    fn single_file_selects_it_in_the_tree() {
        let mut app = App::new(AppOptions {
            root: Some("/p".into()),
            file: Some("/p/a.md".into()),
            ..AppOptions::default()
        });
        app.handle_event(ServiceEvent::Listed {
            dir: "/p".into(),
            result: Ok(vec![entry("/p/docs", true), entry("/p/a.md", false)]),
        });
        assert_eq!(app.view().tree.selected, Some(1));
        assert_eq!(app.view().status.file.as_deref(), Some("a.md"));
    }

    #[test]
    fn config_reload_applies_keys_and_side() {
        use crate::config::{Config, Layer};
        let mut app = App::new(AppOptions::default());
        let (config, errors) = Config::from_layers(&[Layer {
            path: "c.toml".into(),
            text: "[ui]\ntree_side = \"left\"\n[keys.normal]\nQ = \"app.quit\"\n".into(),
        }]);
        app.apply_config(&config, &errors);
        assert_eq!(app.layout().order().next(), Some(PaneId::Tree));
        press(&mut app, "Q");
        assert!(app.should_quit());
    }

    #[test]
    fn reloading_replaces_the_problems_modal() {
        use crate::config::{Config, Layer};
        let bad = |line: &str| {
            Config::from_layers(&[Layer {
                path: "c.toml".into(),
                text: line.into(),
            }])
        };
        let mut app = App::new(AppOptions::default());
        for _ in 0..3 {
            let (config, errors) = bad("[ui]\nbogus = 1\n");
            app.apply_config(&config, &errors);
        }
        assert_eq!(app.modals().len(), 1, "replaced, not stacked");
        let (config, errors) = bad("");
        app.apply_config(&config, &errors);
        assert!(!app.modals().is_open(), "fixed: the stale list goes");
    }

    #[test]
    fn a_clean_reload_clears_old_problems_everywhere() {
        use crate::config::{Config, Layer};
        let layer = |t: &str| Layer {
            path: "c.toml".into(),
            text: t.into(),
        };
        let mut app = App::new(AppOptions::default());
        let (config, errors) = Config::from_layers(&[layer("[ui]\nbogus = 1\n")]);
        app.apply_config(&config, &errors);
        press(&mut app, "<Esc>");
        let (config, errors) = Config::from_layers(&[layer("")]);
        app.apply_config(&config, &errors);
        assert_eq!(app.view().status.message, None);
        press(&mut app, "<leader>o");
        assert!(!app.modals().is_open(), "the stale list can't be reopened");
    }

    #[test]
    fn removed_folders_are_unwatched() {
        let mut app = tree_app();
        press(&mut app, "l");
        app.take_requests();
        app.handle_event(ServiceEvent::Listed {
            dir: "/p/docs".into(),
            result: Ok(vec![]),
        });
        app.handle_event(ServiceEvent::Listed {
            dir: "/p".into(),
            result: Ok(vec![entry("/p/a.md", false)]),
        });
        assert_eq!(
            app.take_requests(),
            vec![Request::Unwatch("/p/docs".into())]
        );
    }

    fn document(path: &str, text: &str) -> crate::io::Document {
        crate::io::Document {
            path: path.into(),
            text: text.into(),
            model: crate::render::RenderModel::default(),
            markdown: true,
        }
    }

    fn opened(app: &mut App, path: &str, text: &str) {
        app.handle_event(ServiceEvent::Opened {
            path: path.into(),
            result: Ok(document(path, text)),
        });
    }

    #[test]
    fn opening_a_file_shows_it_in_the_raw_pane() {
        let mut app = App::new(AppOptions {
            root: Some("/p".into()),
            file: Some("/p/a.md".into()),
            ..AppOptions::default()
        });
        let requests = app.take_requests();
        assert!(requests.contains(&Request::OpenFile {
            path: "/p/a.md".into(),
            flavor: None
        }));
        assert_eq!(app.view().raw.message.as_deref(), Some("Loading…"));
        opened(&mut app, "/p/a.md", "# Title\nbody\n");
        let view = app.view();
        assert_eq!(view.raw.title.as_deref(), Some("a.md"));
        assert_eq!(
            view.raw.lines.iter().map(|l| l.1).collect::<Vec<_>>(),
            vec!["# Title", "body"]
        );
        assert_eq!(view.raw.lines[0].0, 1);
    }

    #[test]
    fn a_late_open_for_another_file_is_dropped() {
        let mut app = App::new(AppOptions {
            file: Some("/p/b.md".into()),
            ..AppOptions::default()
        });
        opened(&mut app, "/p/a.md", "old");
        assert!(app.view().raw.lines.is_empty());
    }

    #[test]
    fn raw_pane_scrolls_and_keeps_its_place_on_reload() {
        let mut app = App::new(AppOptions {
            file: Some("/p/a.md".into()),
            ..AppOptions::default()
        });
        app.resize(80, 12);
        let text: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        opened(&mut app, "/p/a.md", &text);
        press(&mut app, "5j");
        assert_eq!(app.view().raw.lines[0].0, 6);
        press(&mut app, "<C-d>");
        assert_eq!(
            app.view().raw.lines[0].0,
            10,
            "half of the 9 rows the pane shows"
        );
        press(&mut app, "G");
        assert_eq!(
            app.view().raw.lines[0].0,
            92,
            "the last page stays full: 92-100"
        );
        press(&mut app, "gg");
        press(&mut app, "<C-f>");
        assert_eq!(app.view().raw.lines[0].0, 10);
        // A change on disk asks for it again; the new text keeps the place.
        app.take_requests();
        app.handle_event(ServiceEvent::Changed(vec!["/p/a.md".into()]));
        assert_eq!(app.take_requests().len(), 1);
        opened(&mut app, "/p/a.md", &text);
        assert_eq!(app.view().raw.lines[0].0, 10);
    }

    #[test]
    fn open_errors_show_in_the_pane_and_reload_errors_in_the_status() {
        let mut app = App::new(AppOptions {
            file: Some("/p/a.md".into()),
            ..AppOptions::default()
        });
        app.handle_event(ServiceEvent::Opened {
            path: "/p/a.md".into(),
            result: Err("binary file".into()),
        });
        assert_eq!(
            app.view().raw.message.as_deref(),
            Some("can't open a.md: binary file")
        );
        opened(&mut app, "/p/a.md", "text");
        app.handle_event(ServiceEvent::Opened {
            path: "/p/a.md".into(),
            result: Err("gone".into()),
        });
        assert_eq!(app.view().raw.lines.len(), 1, "what was shown stays");
        assert_eq!(
            app.view().status.message.as_deref(),
            Some("couldn't reload a.md: gone")
        );
    }

    #[test]
    fn counts_on_g_and_gg_go_to_that_line() {
        let mut app = App::new(AppOptions {
            file: Some("/p/a.md".into()),
            ..AppOptions::default()
        });
        let text: String = (1..=100).map(|i| format!("{i}\n")).collect();
        opened(&mut app, "/p/a.md", &text);
        press(&mut app, "50G");
        assert_eq!(app.view().raw.lines[0].0, 50);
        press(&mut app, "10gg");
        assert_eq!(app.view().raw.lines[0].0, 10);
    }

    #[test]
    fn a_reload_keeps_the_session_line_numbers_choice() {
        use crate::config::{Config, Layer};
        let mut app = App::new(AppOptions::default());
        press(&mut app, "<leader>n");
        let (config, errors) = Config::from_layers(&[Layer {
            path: "c.toml".into(),
            text: "[ui]\ntree_side = \"left\"\n".into(),
        }]);
        app.apply_config(&config, &errors);
        assert!(
            !app.view().raw.line_numbers,
            "unrelated reload keeps it off"
        );
        let (config, errors) = Config::from_layers(&[Layer {
            path: "c.toml".into(),
            text: "[editor.line_numbers]\nmode = \"off\"\n".into(),
        }]);
        app.apply_config(&config, &errors);
        let (config, errors) = Config::from_layers(&[]);
        app.apply_config(&config, &errors);
        assert!(
            app.view().raw.line_numbers,
            "the setting itself changed back to on"
        );
    }

    #[test]
    fn a_flavor_change_reparses_the_open_file() {
        use crate::config::{Config, Layer};
        let mut app = App::new(AppOptions {
            file: Some("/p/a.md".into()),
            ..AppOptions::default()
        });
        opened(&mut app, "/p/a.md", "x");
        app.take_requests();
        let (config, errors) = Config::from_layers(&[Layer {
            path: "c.toml".into(),
            text: "[markdown]\nflavor = \"commonmark\"\n".into(),
        }]);
        app.apply_config(&config, &errors);
        assert_eq!(
            app.take_requests(),
            vec![Request::OpenFile {
                path: "/p/a.md".into(),
                flavor: Some("commonmark".into())
            }]
        );
    }

    #[test]
    fn enter_retries_a_failed_open_and_switching_shows_loading() {
        let mut app = tree_app();
        press(&mut app, "j<CR>");
        app.take_requests();
        app.handle_event(ServiceEvent::Opened {
            path: "/p/a.md".into(),
            result: Err("EIO".into()),
        });
        press(&mut app, "<CR>");
        assert_eq!(app.take_requests().len(), 1, "Enter tries again");
        opened(&mut app, "/p/a.md", "text");
        press(&mut app, "<CR>");
        assert!(
            app.take_requests().is_empty(),
            "already showing: nothing to do"
        );
    }

    #[test]
    fn line_numbers_toggle() {
        let mut app = App::new(AppOptions::default());
        assert!(app.view().raw.line_numbers);
        press(&mut app, "<leader>n");
        assert!(!app.view().raw.line_numbers);
    }

    fn search_app() -> App {
        let mut app = App::new(AppOptions {
            file: Some("/p/a.md".into()),
            ..AppOptions::default()
        });
        app.resize(80, 12);
        let text: String = (1..=40)
            .map(|i| {
                if i % 10 == 0 {
                    format!("target {i}\n")
                } else {
                    format!("line {i}\n")
                }
            })
            .collect();
        opened(&mut app, "/p/a.md", &text);
        app
    }

    #[test]
    fn search_types_jumps_and_steps() {
        let mut app = search_app();
        press(&mut app, "/targ");
        assert_eq!(app.view().status.prompt.as_deref(), Some("/targ"));
        assert_eq!(app.view().status.mode, "search");
        press(&mut app, "<CR>");
        assert_eq!(app.view().status.prompt, None);
        assert_eq!(app.view().raw.lines[0].0, 10);
        assert_eq!(app.view().search.unwrap().position, Some((1, 4)));
        press(&mut app, "n");
        assert_eq!(app.view().raw.lines[0].0, 20);
        press(&mut app, "N");
        assert_eq!(app.view().raw.lines[0].0, 10);
        press(&mut app, "3n");
        assert_eq!(
            app.view().raw.lines[0].0,
            32,
            "line 40 at the bottom of the last full page"
        );
        press(&mut app, "n");
        assert_eq!(app.view().search.unwrap().position, Some((1, 4)));
        assert_eq!(
            app.view().status.message.as_deref(),
            Some("search hit the end, continuing at the top")
        );
    }

    #[test]
    fn backward_search_and_its_n() {
        let mut app = search_app();
        press(&mut app, "G?target<CR>");
        assert_eq!(app.view().search.unwrap().current_line, Some(30));
        // n keeps going backward; N goes forward.
        press(&mut app, "n");
        assert_eq!(app.view().search.unwrap().current_line, Some(20));
        press(&mut app, "N");
        assert_eq!(app.view().search.unwrap().current_line, Some(30));
    }

    #[test]
    fn escape_restores_and_an_empty_pattern_repeats() {
        let mut app = search_app();
        press(&mut app, "/target<CR>");
        press(&mut app, "/zz<Esc>");
        assert_eq!(app.view().status.mode, "normal");
        assert_eq!(app.view().search.unwrap().regex.as_str(), "target");
        press(&mut app, "/<CR>");
        assert_eq!(app.view().search.unwrap().current_line, Some(20));
    }

    #[test]
    fn search_typing_takes_digits_spaces_and_edits() {
        let mut app = search_app();
        press(&mut app, "/line 3<BS>4");
        assert_eq!(app.view().status.prompt.as_deref(), Some("/line 4"));
        press(&mut app, "<C-u>target 2<CR>");
        assert_eq!(app.view().search.unwrap().current_line, Some(20));
    }

    #[test]
    fn a_modal_over_the_prompt_takes_the_keys() {
        use crate::config::{Config, Layer};
        let mut app = search_app();
        press(&mut app, "/targ");
        let (config, errors) = Config::from_layers(&[Layer {
            path: "c.toml".into(),
            text: "[ui]\nbogus = 1\n".into(),
        }]);
        app.apply_config(&config, &errors);
        assert!(app.modals().is_open());
        press(&mut app, "<Esc>");
        assert!(!app.modals().is_open(), "Esc closes the modal");
        assert_eq!(app.view().status.mode, "normal");
        assert_eq!(app.view().status.prompt, None);
    }

    #[test]
    fn a_reload_mid_prompt_rescans_the_earlier_search() {
        let mut app = search_app();
        press(&mut app, "/target<CR>/");
        let text: String = (1..=40)
            .map(|i| {
                if i % 10 == 5 {
                    format!("target {i}\n")
                } else {
                    format!("line {i}\n")
                }
            })
            .collect();
        opened(&mut app, "/p/a.md", &text);
        // Esc brings back /target, its matches found in the new text: n goes
        // on from the view (line 10) to the new match on 15.
        press(&mut app, "<Esc>n");
        assert_eq!(app.view().search.unwrap().current_line, Some(15));
        assert_eq!(app.view().search.unwrap().position, Some((2, 4)));
    }

    #[test]
    fn n_searches_from_where_the_view_was_scrolled() {
        let mut app = search_app();
        press(&mut app, "/target<CR>");
        assert_eq!(app.view().search.unwrap().current_line, Some(10));
        press(&mut app, "25gg");
        press(&mut app, "n");
        assert_eq!(app.view().search.unwrap().current_line, Some(30));
        press(&mut app, "gg15jN");
        assert_eq!(app.view().search.unwrap().current_line, Some(10));
    }

    #[test]
    fn a_match_deep_in_a_long_line_is_on_screen() {
        let mut app = App::new(AppOptions {
            file: Some("/p/a.md".into()),
            ..AppOptions::default()
        });
        app.resize(80, 12);
        let long = format!("{} needle end", "word ".repeat(600));
        opened(&mut app, "/p/a.md", &format!("{long}\n\nafter\n"));
        press(&mut app, "/needle<CR>");
        let view = app.view();
        let (_, width) = app.raw_columns();
        let at = long.find("needle").unwrap();
        let row = crate::text::row_at(&long, width, at);
        assert!(row > 20, "the match is deep in the line");
        // The match's row is drawn: the first row shown is at most it.
        assert!(view.raw.first_row <= row && row < view.raw.first_row + 10);
    }

    #[test]
    fn keys_of_an_unfinished_binding_type_themselves() {
        use crate::config::{Config, Layer};
        let mut app = search_app();
        let (config, errors) = Config::from_layers(&[Layer {
            path: "c.toml".into(),
            text: "[keys.search]\njk = \"search.cancel\"\n".into(),
        }]);
        assert!(errors.is_empty(), "{errors:?}");
        app.apply_config(&config, &errors);
        press(&mut app, "/major");
        assert_eq!(app.view().status.prompt.as_deref(), Some("/major"));
        press(&mut app, "j");
        app.handle_timeout();
        assert_eq!(app.view().status.prompt.as_deref(), Some("/majorj"));
        press(&mut app, "jk");
        assert_eq!(app.view().status.mode, "normal");
    }

    #[test]
    fn search_insert_can_be_bound_with_text() {
        use crate::config::{Config, Layer};
        let mut app = search_app();
        let (config, errors) = Config::from_layers(&[Layer {
            path: "c.toml".into(),
            text: "[keys.search]\n\"<C-t>\" = \"search.insert target 3\"\n\"<C-y>\" = \"search.insert\"\n".into(),
        }]);
        app.apply_config(&config, &errors);
        assert!(app.modals().is_open(), "a bare search.insert is reported");
        press(&mut app, "<Esc>/<C-t>");
        assert_eq!(app.view().status.prompt.as_deref(), Some("/target 3"));
    }

    #[test]
    fn missing_patterns_and_clearing() {
        let mut app = search_app();
        press(&mut app, "/nothing<CR>");
        assert_eq!(
            app.view().status.message.as_deref(),
            Some("pattern not found: nothing")
        );
        press(&mut app, "/target<CR><leader>h");
        assert!(app.view().search.is_none(), "the lights go");
        // The search stays, as after Vim's :noh: n goes on and lights again.
        press(&mut app, "n");
        assert_eq!(app.view().search.unwrap().current_line, Some(20));
        let mut fresh = search_app();
        press(&mut fresh, "n");
        assert!(
            fresh
                .view()
                .status
                .message
                .unwrap()
                .starts_with("no search yet")
        );
    }

    #[test]
    fn a_counted_n_steps_then_places_once() {
        let mut app = search_app();
        press(&mut app, "/target<CR>2n");
        assert_eq!(app.view().search.unwrap().current_line, Some(30));
        press(&mut app, "5n");
        assert_eq!(
            app.view().search.unwrap().current_line,
            Some(40),
            "wraps: 40, 10, 20, 30, 40"
        );
    }

    #[test]
    fn enter_and_esc_after_an_unfinished_binding_still_work() {
        use crate::config::{Config, Layer};
        let mut app = search_app();
        let (config, errors) = Config::from_layers(&[Layer {
            path: "c.toml".into(),
            text: "[keys.search]\njk = \"search.cancel\"\n".into(),
        }]);
        app.apply_config(&config, &errors);
        press(&mut app, "/targej<BS>t<CR>");
        assert_eq!(app.view().status.mode, "normal", "Enter confirmed");
        assert_eq!(app.view().search.unwrap().regex.as_str(), "target");
        press(&mut app, "/xj<Esc>");
        assert_eq!(app.view().status.mode, "normal", "Esc cancelled");
        assert_eq!(app.view().search.unwrap().regex.as_str(), "target");
    }

    #[test]
    fn user_commands_are_not_bound_in_search_mode() {
        let mut registry = crate::action::Registry::with_builtins();
        registry
            .register("user.hello", "Say hello", crate::action::Source::User)
            .unwrap();
        let modes = registry.get("user.hello").unwrap().modes;
        assert!(modes.contains(&Mode::Normal));
        assert!(!modes.contains(&Mode::Search));
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
