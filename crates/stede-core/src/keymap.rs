// SPDX-License-Identifier: GPL-3.0-or-later
//! Keymaps as data, and the resolver that turns key presses into actions.
//!
//! A binding maps a key sequence to an [`Action`] in one mode and one scope:
//! every pane, one pane, or modals. Lookup tries the focused pane's bindings
//! first, then the ones for every pane; while a modal is open only modal
//! bindings apply. The resolver handles counts (`3<C-w>l`), multi-key
//! sequences (`gg`, `<leader>p`), and sequences that are both complete and a
//! prefix of a longer one, which wait for more keys until a timeout.

use std::collections::BTreeMap;
use std::fmt;

use crate::action::{Action, Registry};
use crate::key::{Key, KeyCode, KeyParseError, format_keys, parse_keys};
use crate::layout::PaneId;
use crate::modal::ModalKind;

/// Modes: each has its own bindings. Phase 1 (the reader) edits in Normal
/// only; Probe is the `stede --probe` terminal check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Mode {
    Normal,
    /// `stede --probe`: every key is echoed, and these bindings still apply.
    Probe,
}

impl Mode {
    /// Every mode, including Probe, which isn't an editor mode: code that
    /// lists the editor's modes (cheat sheets, the status line) should use
    /// [`Mode::EDITOR`].
    pub const ALL: [Mode; 2] = [Mode::Normal, Mode::Probe];

    /// The modes the editor itself runs in.
    pub const EDITOR: [Mode; 1] = [Mode::Normal];

    /// The name used in config tables (`[keys.normal]`) and the status line.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Normal => "normal",
            Mode::Probe => "probe",
        }
    }
}

/// Where a binding applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
    /// Every pane.
    Global,
    /// One pane, ahead of the global bindings.
    Pane(PaneId),
    /// Inside any modal. While a modal is open, only modal scopes apply.
    Modal,
    /// Inside output modals only (the cheat sheet, lint lists): reading keys
    /// such as `j`/`k`, which input and picker modals need for typing.
    OutputModal,
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Scope::Global => f.write_str("all panes"),
            Scope::Pane(p) => write!(f, "{} pane", p.title()),
            Scope::Modal => f.write_str("modals"),
            Scope::OutputModal => f.write_str("output modals"),
        }
    }
}

/// What the resolver looks keys up against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Context {
    pub mode: Mode,
    pub pane: PaneId,
    /// The kind of the top modal, if one is open.
    pub modal: Option<ModalKind>,
}

impl Context {
    /// Scopes to search, highest priority first.
    fn scopes(&self) -> Vec<Scope> {
        match self.modal {
            Some(ModalKind::Output) => vec![Scope::OutputModal, Scope::Modal],
            Some(_) => vec![Scope::Modal],
            None => vec![Scope::Pane(self.pane), Scope::Global],
        }
    }
}

/// One binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub mode: Mode,
    pub scope: Scope,
    pub keys: Vec<Key>,
    pub action: Action,
}

/// Why a binding was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindError {
    Keys(String, KeyParseError),
    EmptyAction(String),
    UnknownAction(String),
    /// Normal-mode keys can't start with `1`-`9`: those type a count.
    CountPrefix(String),
    /// An unbind (empty action) named keys that have no binding there.
    NotBound(String),
    /// The action's arguments are wrong (`pane.toggle preview`).
    BadArgs {
        keys: String,
        message: String,
    },
    /// The action doesn't run in this mode (its registry entry says where it does).
    WrongMode {
        keys: String,
        action: String,
        mode: Mode,
        runs_in: Vec<Mode>,
    },
    /// Probe mode has no panes or modals: its bindings are global only.
    WrongScope {
        keys: String,
        scope: Scope,
        mode: Mode,
    },
}

impl fmt::Display for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Keys(keys, e) => write!(f, "bad keys {keys:?}: {e}"),
            Self::EmptyAction(keys) => write!(f, "keys {keys:?} are bound to an empty action"),
            Self::UnknownAction(name) => write!(f, "unknown action {name:?}"),
            Self::WrongMode {
                keys,
                action,
                mode,
                runs_in,
            } => {
                let modes: Vec<&str> = runs_in.iter().map(|m| m.name()).collect();
                write!(
                    f,
                    "keys {keys:?}: {action:?} doesn't run in {} mode (it runs in: {})",
                    mode.name(),
                    modes.join(", ")
                )
            }
            Self::WrongScope { keys, scope, mode } => write!(
                f,
                "keys {keys:?}: {} mode bindings can't be scoped to {scope}",
                mode.name()
            ),
            Self::BadArgs { keys, message } => write!(f, "keys {keys:?}: {message}"),
            Self::NotBound(keys) => write!(
                f,
                "keys {keys:?} aren't bound here, so there's nothing to unbind"
            ),
            Self::CountPrefix(keys) => write!(
                f,
                "keys {keys:?} start with a digit, which types a count in Normal mode"
            ),
        }
    }
}

impl std::error::Error for BindError {}

type Table = BTreeMap<Vec<Key>, Action>;

/// All bindings, by mode and scope.
#[derive(Clone, Debug)]
pub struct Keymap {
    leader: Key,
    tables: BTreeMap<(Mode, Scope), Table>,
}

impl Keymap {
    /// An empty keymap with the given leader key.
    pub fn new(leader: Key) -> Self {
        Self {
            leader,
            tables: BTreeMap::new(),
        }
    }

    /// The built-in Vim-style bindings, with `<Space>` as leader.
    pub fn defaults(registry: &Registry) -> Self {
        Self::defaults_with_leader(registry, Key::char(' '))
    }

    /// The built-in bindings with another leader key.
    pub fn defaults_with_leader(registry: &Registry, leader: Key) -> Self {
        let mut keymap = Self::new(leader);
        // A default the leader makes invalid is left out rather than failing
        // the whole keymap; config rejects such leaders, so this is a backstop.
        for &(mode, scope, keys, action) in DEFAULT_BINDINGS {
            let bound = keymap.bind(mode, scope, keys, action, registry);
            debug_assert!(
                bound.is_ok() || leader != Key::char(' '),
                "default binding {keys:?} is invalid: {bound:?}"
            );
        }
        keymap
    }

    /// The defaults with config bindings applied over them, in order. An
    /// empty action unbinds the keys. Returns the problems, each at the
    /// config entry it came from.
    pub fn from_config(
        registry: &Registry,
        config: &crate::config::Config,
    ) -> (Self, Vec<crate::config::ConfigError>) {
        let mut keymap = Self::defaults_with_leader(registry, config.leader);
        let mut errors = Vec::new();
        for entry in &config.bindings {
            let result = if entry.action.is_empty() {
                keymap
                    .unbind(entry.mode, entry.scope, &entry.keys)
                    .and_then(|found| {
                        if found {
                            Ok(())
                        } else {
                            Err(BindError::NotBound(entry.keys.clone()))
                        }
                    })
            } else {
                keymap.bind(
                    entry.mode,
                    entry.scope,
                    &entry.keys,
                    &entry.action,
                    registry,
                )
            };
            if let Err(e) = result {
                errors.push(entry.origin.error(e.to_string()));
            }
        }
        (keymap, errors)
    }

    /// Removes the binding of `keys` in this mode and scope. Returns whether
    /// there was one.
    pub fn unbind(&mut self, mode: Mode, scope: Scope, keys: &str) -> Result<bool, BindError> {
        let parsed =
            parse_keys(keys, Some(self.leader)).map_err(|e| BindError::Keys(keys.to_owned(), e))?;
        Ok(self
            .tables
            .get_mut(&(mode, scope))
            .is_some_and(|t| t.remove(&parsed).is_some()))
    }

    pub fn leader(&self) -> Key {
        self.leader
    }

    /// Binds `keys` (Vim notation) to `action` (`"name args"`). A later
    /// binding of the same keys in the same mode and scope replaces the
    /// earlier one.
    pub fn bind(
        &mut self,
        mode: Mode,
        scope: Scope,
        keys: &str,
        action: &str,
        registry: &Registry,
    ) -> Result<(), BindError> {
        let parsed =
            parse_keys(keys, Some(self.leader)).map_err(|e| BindError::Keys(keys.to_owned(), e))?;
        let action =
            Action::parse(action).ok_or_else(|| BindError::EmptyAction(keys.to_owned()))?;
        let Some(spec) = registry.get(&action.name) else {
            return Err(BindError::UnknownAction(action.name));
        };
        if !spec.modes.contains(&mode) {
            return Err(BindError::WrongMode {
                keys: keys.to_owned(),
                runs_in: spec.modes.to_vec(),
                action: action.name,
                mode,
            });
        }
        registry
            .check_args(&action)
            .map_err(|message| BindError::BadArgs {
                keys: keys.to_owned(),
                message,
            })?;
        if mode == Mode::Probe && scope != Scope::Global {
            return Err(BindError::WrongScope {
                keys: keys.to_owned(),
                scope,
                mode,
            });
        }
        // A leading 1-9 always starts a count in Normal mode, so such a
        // binding could never run.
        if mode == Mode::Normal
            && matches!(scope, Scope::Global | Scope::Pane(_))
            && parsed[0].digit().is_some_and(|d| d != 0)
        {
            return Err(BindError::CountPrefix(keys.to_owned()));
        }
        self.tables
            .entry((mode, scope))
            .or_default()
            .insert(parsed, action);
        Ok(())
    }

    /// The first keys (in key order) bound to `action` in this mode and scope.
    pub fn keys_for(&self, mode: Mode, scope: Scope, action: &str) -> Option<Vec<Key>> {
        self.tables
            .get(&(mode, scope))?
            .iter()
            .find(|(_, a)| a.name == action)
            .map(|(keys, _)| keys.clone())
    }

    /// Every binding, by mode, then scope, then keys.
    pub fn bindings(&self) -> impl Iterator<Item = Binding> + '_ {
        self.tables.iter().flat_map(|(&(mode, scope), table)| {
            table.iter().map(move |(keys, action)| Binding {
                mode,
                scope,
                keys: keys.clone(),
                action: action.clone(),
            })
        })
    }

    /// The action bound to exactly `keys` in this context, honoring scope priority.
    ///
    /// Shadowing: the highest-priority scope with any binding that equals or
    /// starts with `keys` owns them. Lower scopes aren't consulted, so a pane
    /// binding fully overrides global ones that share its prefix.
    fn exact(&self, ctx: Context, keys: &[Key]) -> Option<&Action> {
        self.owner(ctx, keys)?.get(keys)
    }

    /// Whether the scope that owns `keys` has a binding longer than `keys`.
    fn has_longer(&self, ctx: Context, keys: &[Key]) -> bool {
        self.owner(ctx, keys).is_some_and(|table| {
            table
                .range(keys.to_vec()..)
                .take_while(|(k, _)| k.starts_with(keys))
                .any(|(k, _)| k.len() > keys.len())
        })
    }

    /// The table of the first scope, by priority, with a binding equal to or
    /// starting with `keys`.
    fn owner(&self, ctx: Context, keys: &[Key]) -> Option<&Table> {
        ctx.scopes().into_iter().find_map(|scope| {
            let table = self.tables.get(&(ctx.mode, scope))?;
            table
                .range(keys.to_vec()..)
                .next()
                .filter(|(k, _)| k.starts_with(keys))
                .map(|_| table)
        })
    }
}

/// What a key press resolved to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// More keys may follow. Call [`Resolver::timeout`] if none come in time.
    Pending,
    /// A count digit was typed. Wait for the next key; there is no timeout.
    Count,
    /// Run this action, `count` times if a count was typed.
    Run { action: Action, count: Option<u32> },
    /// The keys typed match no binding.
    Unbound(String),
    /// `<Esc>` dropped a pending sequence or count.
    Cancelled,
}

/// The largest count accepted; more digits are ignored.
pub const MAX_COUNT: u32 = 99_999;

/// Turns key presses into actions, one key at a time.
#[derive(Clone, Debug, Default)]
pub struct Resolver {
    pending: Vec<Key>,
    count: Option<u32>,
}

impl Resolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// What has been typed toward a binding so far, such as `3g`.
    pub fn pending(&self) -> String {
        let count = self.count.map(|c| c.to_string()).unwrap_or_default();
        format!("{count}{}", format_keys(&self.pending))
    }

    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty() || self.count.is_some()
    }

    /// Drops anything typed so far.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.count = None;
    }

    /// Feeds one key press.
    pub fn feed(&mut self, keymap: &Keymap, ctx: Context, key: Key) -> Step {
        if key == Key::plain(KeyCode::Esc) && self.is_pending() {
            self.reset();
            // In a modal, Esc is also the close key: one press closes it even
            // with keys half-typed. Elsewhere it only cancels them.
            if ctx.modal.is_none() {
                return Step::Cancelled;
            }
        }
        // Counts: only in Normal mode outside modals, before the first key of a
        // sequence. `0` only continues a count; on its own it can be a motion.
        let count_digit = key
            .digit()
            .filter(|&d| d != 0 || self.count.is_some())
            .filter(|_| ctx.modal.is_none() && ctx.mode == Mode::Normal && self.pending.is_empty());
        if let Some(d) = count_digit {
            let next = self.count.unwrap_or(0).saturating_mul(10).saturating_add(d);
            self.count = Some(next.min(MAX_COUNT));
            return Step::Count;
        }

        self.pending.push(key);
        let longer = keymap.has_longer(ctx, &self.pending);
        if longer {
            return Step::Pending;
        }
        let step = match keymap.exact(ctx, &self.pending) {
            Some(action) => Step::Run {
                action: action.clone(),
                count: self.count,
            },
            // A bare, unbound `<Esc>` is habit, not a mistake: say nothing.
            None if self.pending == [Key::plain(KeyCode::Esc)] => Step::Cancelled,
            None => Step::Unbound(self.pending()),
        };
        self.reset();
        step
    }

    /// No key came in time: run the sequence typed so far if it is a complete
    /// binding, else report it unbound. Returns `None` if nothing was pending.
    pub fn timeout(&mut self, keymap: &Keymap, ctx: Context) -> Option<Step> {
        if self.pending.is_empty() {
            // A count alone doesn't time out: it waits for its key, as in Vim.
            return None;
        }
        let step = match keymap.exact(ctx, &self.pending) {
            Some(action) => Step::Run {
                action: action.clone(),
                count: self.count,
            },
            None => Step::Unbound(self.pending()),
        };
        self.reset();
        Some(step)
    }
}

use Mode::{Normal, Probe};
use Scope::{Global, Modal, OutputModal, Pane};

/// The built-in bindings: mode, scope, keys, action.
///
/// Every action here must be registered; a test checks it.
const DEFAULT_BINDINGS: &[(Mode, Scope, &str, &str)] = &[
    (Normal, Global, "ZZ", "app.quit"),
    (Normal, Global, "ZQ", "app.quit"),
    (Normal, Global, "<C-w>h", "pane.focus_left"),
    (Normal, Global, "<C-w>l", "pane.focus_right"),
    (Normal, Global, "<C-h>", "pane.focus_left"),
    (Normal, Global, "<C-l>", "pane.focus_right"),
    (Normal, Global, "<leader>e", "pane.toggle tree"),
    (Normal, Global, "<leader>p", "pane.toggle rendered"),
    (Normal, Global, "<leader>r", "pane.toggle raw"),
    (Normal, Global, "<leader>z", "pane.focus_one"),
    (Normal, Global, "<leader>?", "help.keys"),
    (Normal, Global, "<F1>", "help.keys"),
    (Normal, Global, "<leader>o", "modal.reopen_last"),
    (Normal, Global, "<leader>c", "config.problems"),
    (Normal, Modal, "<Esc>", "modal.close"),
    (Normal, Modal, "<C-c>", "modal.close_all"),
    (Probe, Global, "qq", "probe.quit"),
    (Normal, Pane(PaneId::Tree), "j", "tree.down"),
    (Normal, Pane(PaneId::Tree), "<Down>", "tree.down"),
    (Normal, Pane(PaneId::Tree), "k", "tree.up"),
    (Normal, Pane(PaneId::Tree), "<Up>", "tree.up"),
    (Normal, Pane(PaneId::Tree), "gg", "tree.top"),
    (Normal, Pane(PaneId::Tree), "G", "tree.bottom"),
    (Normal, Pane(PaneId::Tree), "l", "tree.open"),
    (Normal, Pane(PaneId::Tree), "<CR>", "tree.open"),
    (Normal, Pane(PaneId::Tree), "<Right>", "tree.open"),
    (Normal, Pane(PaneId::Tree), "h", "tree.close"),
    (Normal, Pane(PaneId::Tree), "<Left>", "tree.close"),
    (Normal, OutputModal, "j", "modal.scroll_down"),
    (Normal, OutputModal, "<Down>", "modal.scroll_down"),
    (Normal, OutputModal, "k", "modal.scroll_up"),
    (Normal, OutputModal, "<Up>", "modal.scroll_up"),
    (Normal, Modal, "<leader>?", "help.keys"),
    (Normal, Modal, "<F1>", "help.keys"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Registry, Keymap) {
        let r = Registry::with_builtins();
        let k = Keymap::defaults(&r);
        (r, k)
    }

    fn ctx() -> Context {
        Context {
            mode: Mode::Normal,
            pane: PaneId::Raw,
            modal: None,
        }
    }

    fn feed_str(res: &mut Resolver, k: &Keymap, c: Context, s: &str) -> Vec<Step> {
        parse_keys(s, Some(k.leader()))
            .unwrap()
            .into_iter()
            .map(|key| res.feed(k, c, key))
            .collect()
    }

    fn run(name: &str, args: &[&str], count: Option<u32>) -> Step {
        Step::Run {
            action: Action {
                name: name.into(),
                args: args.iter().map(|s| s.to_string()).collect(),
            },
            count,
        }
    }

    #[test]
    fn defaults_bind_registered_actions() {
        let (r, k) = setup();
        assert!(k.bindings().all(|b| r.contains(&b.action.name)));
        assert_eq!(k.bindings().count(), DEFAULT_BINDINGS.len());
    }

    #[test]
    fn sequences() {
        let (_, k) = setup();
        let mut res = Resolver::new();
        assert_eq!(
            feed_str(&mut res, &k, ctx(), "<C-w>l"),
            vec![Step::Pending, run("pane.focus_right", &[], None)]
        );
        assert_eq!(
            feed_str(&mut res, &k, ctx(), "<leader>e"),
            vec![Step::Pending, run("pane.toggle", &["tree"], None)]
        );
        assert!(!res.is_pending());
    }

    #[test]
    fn counts() {
        let (_, k) = setup();
        let mut res = Resolver::new();
        let steps = feed_str(&mut res, &k, ctx(), "12<C-l>");
        assert_eq!(
            steps.last().unwrap(),
            &run("pane.focus_right", &[], Some(12))
        );
        // `0` alone doesn't start a count.
        assert_eq!(
            res.feed(&k, ctx(), Key::char('0')),
            Step::Unbound("0".into())
        );
        // Counts cap rather than overflow.
        let steps = feed_str(&mut res, &k, ctx(), "99999999999<C-l>");
        assert_eq!(
            steps.last().unwrap(),
            &run("pane.focus_right", &[], Some(MAX_COUNT))
        );
    }

    #[test]
    fn unbound_and_cancel() {
        let (_, k) = setup();
        let mut res = Resolver::new();
        assert_eq!(
            feed_str(&mut res, &k, ctx(), "<C-w>x"),
            vec![Step::Pending, Step::Unbound("<C-w>x".into())]
        );
        assert_eq!(
            feed_str(&mut res, &k, ctx(), "3<C-w><Esc>"),
            vec![Step::Count, Step::Pending, Step::Cancelled]
        );
        assert!(!res.is_pending());
        // A bare Esc with nothing pending is quiet.
        assert_eq!(
            res.feed(&k, ctx(), Key::plain(KeyCode::Esc)),
            Step::Cancelled
        );
    }

    #[test]
    fn a_count_survives_a_pause() {
        let (_, k) = setup();
        let mut res = Resolver::new();
        assert_eq!(res.feed(&k, ctx(), Key::char('3')), Step::Count);
        assert_eq!(res.timeout(&k, ctx()), None);
        assert_eq!(
            res.feed(&k, ctx(), Key::ctrl('l')),
            run("pane.focus_right", &[], Some(3))
        );
    }

    #[test]
    fn digit_leading_bindings_are_rejected() {
        let r = Registry::with_builtins();
        let mut k = Keymap::new(Key::char(' '));
        assert_eq!(
            k.bind(Mode::Normal, Scope::Global, "1", "app.quit", &r),
            Err(BindError::CountPrefix("1".into()))
        );
        assert!(
            k.bind(Mode::Normal, Scope::Global, "0", "app.quit", &r)
                .is_ok()
        );
        assert!(
            k.bind(Mode::Normal, Scope::Modal, "1", "app.quit", &r)
                .is_ok()
        );
    }

    #[test]
    fn ambiguous_prefix_waits_then_times_out() {
        let r = Registry::with_builtins();
        let mut k = Keymap::new(Key::char(' '));
        k.bind(Mode::Normal, Scope::Global, "g", "help.keys", &r)
            .unwrap();
        k.bind(Mode::Normal, Scope::Global, "gq", "app.quit", &r)
            .unwrap();
        let mut res = Resolver::new();
        assert_eq!(res.feed(&k, ctx(), Key::char('g')), Step::Pending);
        assert_eq!(res.pending(), "g");
        assert_eq!(res.timeout(&k, ctx()), Some(run("help.keys", &[], None)));
        assert_eq!(res.timeout(&k, ctx()), None);
        // The longer sequence still works when typed in time.
        assert_eq!(
            feed_str(&mut res, &k, ctx(), "gq"),
            vec![Step::Pending, run("app.quit", &[], None)]
        );
    }

    #[test]
    fn pane_bindings_shadow_global_ones() {
        let r = Registry::with_builtins();
        let mut k = Keymap::defaults(&r);
        k.bind(
            Mode::Normal,
            Scope::Pane(PaneId::Tree),
            "<C-l>",
            "help.keys",
            &r,
        )
        .unwrap();
        let mut res = Resolver::new();
        let tree = Context {
            pane: PaneId::Tree,
            ..ctx()
        };
        assert_eq!(
            res.feed(&k, tree, Key::ctrl('l')),
            run("help.keys", &[], None)
        );
        assert_eq!(
            res.feed(&k, ctx(), Key::ctrl('l')),
            run("pane.focus_right", &[], None)
        );
    }

    #[test]
    fn pane_bindings_own_their_prefix() {
        let r = Registry::with_builtins();
        let mut k = Keymap::defaults(&r);
        k.bind(
            Mode::Normal,
            Scope::Pane(PaneId::Tree),
            "<C-w>",
            "help.keys",
            &r,
        )
        .unwrap();
        let tree = Context {
            pane: PaneId::Tree,
            ..ctx()
        };
        let mut res = Resolver::new();
        // Runs at once: the global <C-w>h / <C-w>l don't make it wait.
        assert_eq!(
            res.feed(&k, tree, Key::ctrl('w')),
            run("help.keys", &[], None)
        );
        // Other panes still get the global sequences.
        assert_eq!(
            feed_str(&mut res, &k, ctx(), "<C-w>l"),
            vec![Step::Pending, run("pane.focus_right", &[], None)]
        );
    }

    #[test]
    fn esc_closes_a_modal_even_with_keys_pending() {
        let (_, k) = setup();
        let modal = Context {
            modal: Some(ModalKind::Output),
            ..ctx()
        };
        let mut res = Resolver::new();
        // A stray leader press inside the cheat sheet...
        assert_eq!(res.feed(&k, modal, Key::char(' ')), Step::Pending);
        // ...doesn't stop one Esc from closing it.
        assert_eq!(
            res.feed(&k, modal, Key::plain(KeyCode::Esc)),
            run("modal.close", &[], None)
        );
    }

    #[test]
    fn modal_context_uses_modal_bindings_only() {
        let (_, k) = setup();
        let mut res = Resolver::new();
        let modal = Context {
            modal: Some(ModalKind::Output),
            ..ctx()
        };
        assert_eq!(
            res.feed(&k, modal, Key::plain(KeyCode::Esc)),
            run("modal.close", &[], None)
        );
        assert_eq!(
            res.feed(&k, modal, Key::ctrl('l')),
            Step::Unbound("<C-l>".into())
        );
        // No counts inside modals.
        assert_eq!(
            res.feed(&k, modal, Key::char('3')),
            Step::Unbound("3".into())
        );
    }

    #[test]
    fn reading_keys_only_apply_in_output_modals() {
        let (_, k) = setup();
        let mut res = Resolver::new();
        let output = Context {
            modal: Some(ModalKind::Output),
            ..ctx()
        };
        let input = Context {
            modal: Some(ModalKind::Input),
            ..ctx()
        };
        assert_eq!(
            res.feed(&k, output, Key::char('j')),
            run("modal.scroll_down", &[], None)
        );
        assert_eq!(
            res.feed(&k, input, Key::char('j')),
            Step::Unbound("j".into())
        );
        // The close key works in every modal.
        assert_eq!(
            res.feed(&k, input, Key::plain(KeyCode::Esc)),
            run("modal.close", &[], None)
        );
    }

    #[test]
    fn actions_bind_only_in_the_modes_they_run_in() {
        let mut r = Registry::with_builtins();
        let mut k = Keymap::new(Key::char(' '));
        assert!(matches!(
            k.bind(Mode::Normal, Scope::Global, "Q", "probe.quit", &r),
            Err(BindError::WrongMode {
                mode: Mode::Normal,
                ..
            })
        ));
        let err = k
            .bind(Mode::Probe, Scope::Global, "x", "app.quit", &r)
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "keys \"x\": \"app.quit\" doesn't run in probe mode (it runs in: normal)"
        );
        assert!(
            k.bind(Mode::Probe, Scope::Global, "<C-q>", "probe.quit", &r)
                .is_ok()
        );
        // A user command named probe.* is an editor command like any other:
        // modes come from the registry, not the name.
        r.register("probe.colors", "Show colors", crate::action::Source::User)
            .unwrap();
        assert!(
            k.bind(Mode::Normal, Scope::Global, "gc", "probe.colors", &r)
                .is_ok()
        );
    }

    #[test]
    fn probe_bindings_are_global_only() {
        let r = Registry::with_builtins();
        let mut k = Keymap::new(Key::char(' '));
        assert!(matches!(
            k.bind(Mode::Probe, Scope::Pane(PaneId::Raw), "q", "probe.quit", &r),
            Err(BindError::WrongScope { .. })
        ));
    }

    #[test]
    fn bad_arguments_are_caught_at_bind_time() {
        let r = Registry::with_builtins();
        let mut k = Keymap::new(Key::char(' '));
        assert!(matches!(
            k.bind(Mode::Normal, Scope::Global, "x", "pane.toggle preview", &r),
            Err(BindError::BadArgs { .. })
        ));
        assert!(matches!(
            k.bind(Mode::Normal, Scope::Global, "x", "pane.toggle", &r),
            Err(BindError::BadArgs { .. })
        ));
        assert!(matches!(
            k.bind(Mode::Normal, Scope::Global, "x", "app.quit now", &r),
            Err(BindError::BadArgs { .. })
        ));
        assert!(
            k.bind(Mode::Normal, Scope::Global, "x", "pane.toggle tree", &r)
                .is_ok()
        );
    }

    #[test]
    fn bind_errors() {
        let r = Registry::with_builtins();
        let mut k = Keymap::new(Key::char(' '));
        assert!(matches!(
            k.bind(Mode::Normal, Scope::Global, "<Nope>", "app.quit", &r),
            Err(BindError::Keys(..))
        ));
        assert_eq!(
            k.bind(Mode::Normal, Scope::Global, "x", "no.such", &r),
            Err(BindError::UnknownAction("no.such".into()))
        );
        assert_eq!(
            k.bind(Mode::Normal, Scope::Global, "x", "  ", &r),
            Err(BindError::EmptyAction("x".into()))
        );
    }
}
