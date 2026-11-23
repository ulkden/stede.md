// SPDX-License-Identifier: GPL-3.0-or-later
//! The layout model: a row of panes, each open, folded to a strip, or off.
//!
//! Nothing closes for good: a folded pane is one key (or click) from coming
//! back. Moving focus into a strip opens it. `pane.focus_one` folds every pane
//! but the focused one and, pressed again, restores the layout it saved.
//!
//! Widths are computed here in cells, so every front end lays panes out the
//! same way. Animation is applied by the front end on top of these targets.
//!
//! The modal stack lives here too, next to the panes it opens over.

use std::fmt;
use std::str::FromStr;

use crate::modal::ModalStack;

/// The panes in v1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PaneId {
    /// The raw Markdown.
    Raw,
    /// The rendered, read-only view.
    Rendered,
    /// The file tree.
    Tree,
}

impl PaneId {
    pub const ALL: [PaneId; 3] = [PaneId::Raw, PaneId::Rendered, PaneId::Tree];

    /// The name used in config and actions (`pane.toggle tree`).
    pub fn name(self) -> &'static str {
        match self {
            PaneId::Raw => "raw",
            PaneId::Rendered => "rendered",
            PaneId::Tree => "tree",
        }
    }

    /// The title shown on the pane and its strip.
    pub fn title(self) -> &'static str {
        match self {
            PaneId::Raw => "Raw",
            PaneId::Rendered => "Rendered",
            PaneId::Tree => "Files",
        }
    }
}

impl fmt::Display for PaneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for PaneId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        PaneId::ALL
            .into_iter()
            .find(|p| p.name() == s)
            .ok_or_else(|| format!("unknown pane {s:?}; expected raw, rendered or tree"))
    }
}

/// Whether a pane shows, sits folded at its edge, or is turned off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneState {
    Open,
    Folded,
    Off,
}

/// Which side the file tree sits on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TreeSide {
    Left,
    #[default]
    Right,
}

/// How the layout starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutOptions {
    pub tree_side: TreeSide,
    /// Start with the tree folded, as when a single file is opened.
    pub tree_folded: bool,
    /// Width of a folded pane's strip, in cells.
    pub strip_width: u16,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            tree_side: TreeSide::Right,
            tree_folded: false,
            strip_width: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slot {
    id: PaneId,
    state: PaneState,
    /// Relative share of the width when open.
    weight: u16,
}

/// Where one pane lands on screen, in cells from the left edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneRect {
    pub id: PaneId,
    pub x: u16,
    pub width: u16,
    pub folded: bool,
}

/// The row of panes, their states, and which one has focus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    slots: Vec<Slot>,
    focus: PaneId,
    strip_width: u16,
    /// States saved by `focus_one`, restored when it is pressed again.
    saved: Option<Vec<PaneState>>,
    modals: ModalStack,
}

impl Default for Layout {
    fn default() -> Self {
        Self::new(LayoutOptions::default())
    }
}

impl Layout {
    pub fn new(options: LayoutOptions) -> Self {
        let tree = Slot {
            id: PaneId::Tree,
            state: if options.tree_folded {
                PaneState::Folded
            } else {
                PaneState::Open
            },
            weight: 1,
        };
        let raw = Slot {
            id: PaneId::Raw,
            state: PaneState::Open,
            weight: 2,
        };
        let rendered = Slot {
            id: PaneId::Rendered,
            state: PaneState::Open,
            weight: 2,
        };
        let slots = match options.tree_side {
            TreeSide::Right => vec![raw, rendered, tree],
            TreeSide::Left => vec![tree, raw, rendered],
        };
        Self {
            slots,
            focus: PaneId::Raw,
            strip_width: options.strip_width.max(1),
            saved: None,
            modals: ModalStack::new(),
        }
    }

    /// Moves the file tree to the left or right edge, keeping every pane's
    /// state, width and the focus.
    pub fn set_tree_side(&mut self, side: TreeSide) {
        let i = self.index(PaneId::Tree);
        let tree = self.slots.remove(i);
        match side {
            TreeSide::Left => self.slots.insert(0, tree),
            TreeSide::Right => self.slots.push(tree),
        }
    }

    /// The pane with focus. It is always open.
    pub fn focus(&self) -> PaneId {
        self.focus
    }

    pub fn state(&self, id: PaneId) -> PaneState {
        self.slot(id).state
    }

    /// The panes in screen order.
    pub fn order(&self) -> impl Iterator<Item = PaneId> + '_ {
        self.slots.iter().map(|s| s.id)
    }

    /// Whether `focus_one` has a layout saved to restore.
    pub fn is_focused_one(&self) -> bool {
        self.saved.is_some()
    }

    /// The modals open over the panes.
    pub fn modals(&self) -> &ModalStack {
        &self.modals
    }

    pub fn modals_mut(&mut self) -> &mut ModalStack {
        &mut self.modals
    }

    /// Gives focus to a pane, opening it if it is folded or off.
    ///
    /// Peeking into a strip this way keeps a `focus_one` layout saved, so
    /// `focus_one` still restores it afterwards.
    pub fn focus_pane(&mut self, id: PaneId) {
        let i = self.index(id);
        self.slots[i].state = PaneState::Open;
        self.focus = id;
    }

    /// Moves focus `times` panes left (`step < 0`) or right (`step > 0`),
    /// skipping panes that are off, and stopping at the edge. Only the pane
    /// focus lands on opens; folded panes passed on the way stay folded.
    /// Returns false if focus didn't move.
    pub fn focus_step(&mut self, step: i32, times: u32) -> bool {
        let dir = step.signum();
        if dir == 0 {
            return false;
        }
        let mut i = self.index(self.focus) as i32;
        let mut target = None;
        for _ in 0..times.max(1) {
            let mut j = i + dir;
            while j >= 0
                && (j as usize) < self.slots.len()
                && self.slots[j as usize].state == PaneState::Off
            {
                j += dir;
            }
            if j < 0 || j as usize >= self.slots.len() {
                break;
            }
            i = j;
            target = Some(self.slots[j as usize].id);
        }
        match target {
            Some(id) => {
                self.focus_pane(id);
                true
            }
            None => false,
        }
    }

    /// Folds an open pane or opens a folded (or off) one.
    ///
    /// The last open pane never folds. When the focused pane folds, focus
    /// moves to the nearest open pane. Returns false if nothing changed.
    pub fn toggle(&mut self, id: PaneId) -> bool {
        match self.state(id) {
            PaneState::Open => {
                if self.open_count() <= 1 {
                    return false;
                }
                self.set_state(id, PaneState::Folded);
                if self.focus == id {
                    self.focus = self.nearest_open(id).expect("another pane is open");
                }
                true
            }
            PaneState::Folded | PaneState::Off => {
                self.set_state(id, PaneState::Open);
                true
            }
        }
    }

    /// Folds every pane but the focused one; called again, restores the layout
    /// from before.
    pub fn focus_one(&mut self) {
        if let Some(saved) = self.saved.take() {
            for (slot, state) in self.slots.iter_mut().zip(saved) {
                slot.state = state;
            }
            // The focused pane may have been folded in the saved layout.
            if self.state(self.focus) != PaneState::Open {
                self.set_state(self.focus, PaneState::Open);
            }
            return;
        }
        self.saved = Some(self.slots.iter().map(|s| s.state).collect());
        let focus = self.focus;
        for slot in &mut self.slots {
            slot.state = if slot.id == focus {
                PaneState::Open
            } else if slot.state == PaneState::Off {
                PaneState::Off
            } else {
                PaneState::Folded
            };
        }
    }

    /// Lays the panes out across `width` cells.
    ///
    /// Folded panes take `strip_width` each; open panes share the rest by
    /// weight. Panes that are off take no space and are not listed. If the
    /// width is too small even for the strips, panes get what is left, in order.
    pub fn geometry(&self, width: u16) -> Vec<PaneRect> {
        let visible: Vec<&Slot> = self
            .slots
            .iter()
            .filter(|s| s.state != PaneState::Off)
            .collect();
        let strips = visible
            .iter()
            .filter(|s| s.state == PaneState::Folded)
            .count() as u32;
        let open_weight: u32 = visible
            .iter()
            .filter(|s| s.state == PaneState::Open)
            .map(|s| u32::from(s.weight))
            .sum();
        let for_open = u32::from(width).saturating_sub(strips * u32::from(self.strip_width));

        let mut rects = Vec::with_capacity(visible.len());
        let mut x: u32 = 0;
        let mut given_open: u32 = 0;
        let mut weight_seen: u32 = 0;
        for slot in visible {
            let want = match slot.state {
                PaneState::Folded => u32::from(self.strip_width),
                _ => {
                    // Cumulative rounding: the last open pane absorbs the remainder.
                    weight_seen += u32::from(slot.weight);
                    let upto = for_open * weight_seen / open_weight.max(1);
                    let w = upto - given_open;
                    given_open = upto;
                    w
                }
            };
            let w = want.min(u32::from(width).saturating_sub(x));
            rects.push(PaneRect {
                id: slot.id,
                x: x as u16,
                width: w as u16,
                folded: slot.state == PaneState::Folded,
            });
            x += w;
        }
        self.keep_focus_visible(&mut rects, width);
        rects
    }

    /// When cells are too scarce for everyone, the focused pane still gets at
    /// least one, taken from the widest other pane.
    fn keep_focus_visible(&self, rects: &mut [PaneRect], width: u16) {
        if width == 0 {
            return;
        }
        let Some(fi) = rects.iter().position(|r| r.id == self.focus) else {
            return;
        };
        if rects[fi].width > 0 {
            return;
        }
        let donor = (0..rects.len())
            .filter(|&i| i != fi && rects[i].width > 0)
            .max_by_key(|&i| rects[i].width);
        if let Some(di) = donor {
            rects[di].width -= 1;
            rects[fi].width = 1;
        } else {
            rects[fi].width = 1.min(width);
        }
        let mut x = 0;
        for r in rects.iter_mut() {
            r.x = x;
            x += r.width;
        }
    }

    fn slot(&self, id: PaneId) -> &Slot {
        &self.slots[self.index(id)]
    }

    fn index(&self, id: PaneId) -> usize {
        self.slots
            .iter()
            .position(|s| s.id == id)
            .expect("every pane has a slot")
    }

    fn set_state(&mut self, id: PaneId, state: PaneState) {
        let i = self.index(id);
        self.slots[i].state = state;
        // A manual change ends `focus_one`: there is no longer one layout to restore.
        self.saved = None;
    }

    fn open_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| s.state == PaneState::Open)
            .count()
    }

    fn nearest_open(&self, id: PaneId) -> Option<PaneId> {
        let here = self.index(id);
        (1..self.slots.len())
            .flat_map(|d| [here.checked_sub(d), Some(here + d)])
            .flatten()
            .filter(|&i| i < self.slots.len())
            .map(|i| self.slots[i].id)
            .find(|&p| self.state(p) == PaneState::Open)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(l: &Layout) -> Vec<PaneId> {
        l.order().collect()
    }

    #[test]
    fn default_order_and_focus() {
        let l = Layout::default();
        assert_eq!(ids(&l), vec![PaneId::Raw, PaneId::Rendered, PaneId::Tree]);
        assert_eq!(l.focus(), PaneId::Raw);
        let left = Layout::new(LayoutOptions {
            tree_side: TreeSide::Left,
            ..LayoutOptions::default()
        });
        assert_eq!(
            ids(&left),
            vec![PaneId::Tree, PaneId::Raw, PaneId::Rendered]
        );
    }

    #[test]
    fn single_file_starts_with_tree_folded() {
        let l = Layout::new(LayoutOptions {
            tree_folded: true,
            ..LayoutOptions::default()
        });
        assert_eq!(l.state(PaneId::Tree), PaneState::Folded);
    }

    #[test]
    fn toggle_never_folds_the_last_open_pane() {
        let mut l = Layout::default();
        assert!(l.toggle(PaneId::Rendered));
        assert!(l.toggle(PaneId::Tree));
        assert!(!l.toggle(PaneId::Raw));
        assert_eq!(l.state(PaneId::Raw), PaneState::Open);
    }

    #[test]
    fn folding_the_focused_pane_moves_focus() {
        let mut l = Layout::default();
        l.focus_pane(PaneId::Rendered);
        assert!(l.toggle(PaneId::Rendered));
        assert_eq!(l.state(PaneId::Rendered), PaneState::Folded);
        assert_ne!(l.focus(), PaneId::Rendered);
        assert_eq!(l.state(l.focus()), PaneState::Open);
    }

    #[test]
    fn moving_into_a_strip_opens_it() {
        let mut l = Layout::default();
        l.toggle(PaneId::Rendered);
        assert!(l.focus_step(1, 1));
        assert_eq!(l.focus(), PaneId::Rendered);
        assert_eq!(l.state(PaneId::Rendered), PaneState::Open);
        assert!(l.focus_step(1, 1));
        assert_eq!(l.focus(), PaneId::Tree);
        assert!(!l.focus_step(1, 1), "right edge");
        assert!(l.focus_step(-1, 1));
        assert!(l.focus_step(-1, 1));
        assert!(!l.focus_step(-1, 1), "left edge");
        assert_eq!(l.focus(), PaneId::Raw);
    }

    #[test]
    fn counted_moves_open_only_the_target() {
        let mut l = Layout::default();
        l.toggle(PaneId::Rendered);
        l.toggle(PaneId::Tree);
        assert!(l.focus_step(1, 2));
        assert_eq!(l.focus(), PaneId::Tree);
        assert_eq!(l.state(PaneId::Rendered), PaneState::Folded);
        // Past the edge stops at the last pane.
        assert!(l.focus_step(-1, 9));
        assert_eq!(l.focus(), PaneId::Raw);
    }

    #[test]
    fn peeking_keeps_focus_one_restorable() {
        let mut l = Layout::default();
        l.focus_one();
        assert!(l.focus_step(1, 1), "peek into the Rendered strip");
        assert_eq!(l.state(PaneId::Rendered), PaneState::Open);
        assert!(l.is_focused_one());
        l.focus_one();
        assert!(!l.is_focused_one());
        for id in PaneId::ALL {
            assert_eq!(l.state(id), PaneState::Open, "{id}");
        }
        assert_eq!(l.focus(), PaneId::Rendered);
    }

    #[test]
    fn focus_one_and_restore() {
        let mut l = Layout::default();
        l.toggle(PaneId::Tree);
        l.focus_pane(PaneId::Rendered);
        l.focus_one();
        assert!(l.is_focused_one());
        assert_eq!(l.state(PaneId::Raw), PaneState::Folded);
        assert_eq!(l.state(PaneId::Rendered), PaneState::Open);
        assert_eq!(l.state(PaneId::Tree), PaneState::Folded);
        l.focus_one();
        assert!(!l.is_focused_one());
        assert_eq!(l.state(PaneId::Raw), PaneState::Open);
        assert_eq!(
            l.state(PaneId::Tree),
            PaneState::Folded,
            "restored as it was"
        );
    }

    #[test]
    fn manual_change_ends_focus_one() {
        let mut l = Layout::default();
        l.focus_one();
        l.toggle(PaneId::Tree);
        assert!(!l.is_focused_one());
        // Pressing it again now starts a new focus-one instead of restoring.
        l.focus_one();
        assert!(l.is_focused_one());
        assert_eq!(l.state(PaneId::Tree), PaneState::Folded);
    }

    #[test]
    fn geometry_shares_width_by_weight() {
        let l = Layout::default();
        let g = l.geometry(100);
        assert_eq!(g.iter().map(|r| r.width).sum::<u16>(), 100);
        assert_eq!(
            g[0],
            PaneRect {
                id: PaneId::Raw,
                x: 0,
                width: 40,
                folded: false
            }
        );
        assert_eq!(
            g[1],
            PaneRect {
                id: PaneId::Rendered,
                x: 40,
                width: 40,
                folded: false
            }
        );
        assert_eq!(
            g[2],
            PaneRect {
                id: PaneId::Tree,
                x: 80,
                width: 20,
                folded: false
            }
        );
    }

    #[test]
    fn geometry_with_strips_and_remainders() {
        let mut l = Layout::default();
        l.focus_one();
        let g = l.geometry(81);
        assert_eq!(g[0].width, 79);
        assert!(g[1].folded && g[1].width == 1 && g[1].x == 79);
        assert!(g[2].folded && g[2].width == 1 && g[2].x == 80);

        let l = Layout::default();
        let g = l.geometry(7);
        assert_eq!(g.iter().map(|r| r.width).sum::<u16>(), 7);
    }

    #[test]
    fn geometry_survives_tiny_widths() {
        let mut l = Layout::default();
        l.focus_one();
        for width in 0..4 {
            let g = l.geometry(width);
            assert!(g.iter().map(|r| r.width).sum::<u16>() <= width);
            assert!(g.iter().all(|r| r.x <= width));
        }
    }

    #[test]
    fn focused_pane_stays_visible_when_cramped() {
        let mut l = Layout::default();
        for width in 1..6 {
            let g = l.geometry(width);
            let focus = g.iter().find(|r| r.id == PaneId::Raw).unwrap();
            assert!(focus.width >= 1, "width {width}");
            assert!(g.iter().map(|r| r.width).sum::<u16>() <= width);
        }
        l.focus_one();
        let g = l.geometry(2);
        assert_eq!(g.iter().find(|r| r.id == PaneId::Raw).unwrap().width, 1);
        // Rects stay contiguous after the reshuffle.
        for pair in g.windows(2) {
            assert_eq!(pair[0].x + pair[0].width, pair[1].x);
        }
    }

    #[test]
    fn pane_names_parse() {
        assert_eq!("tree".parse::<PaneId>(), Ok(PaneId::Tree));
        assert!("files".parse::<PaneId>().is_err());
    }
}
