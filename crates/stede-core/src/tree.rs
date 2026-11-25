// SPDX-License-Identifier: GPL-3.0-or-later
//! The file tree's state: listings, which folders are open, the selection.
//!
//! Listings arrive from the services (see [`crate::io`]); the tree only
//! asks for them. Rows are the visible entries in order, with open folders'
//! children nested under them.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::io::Entry;

/// A folder's listing, or where it is in arriving.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Listing {
    Loading,
    Loaded(Vec<Entry>),
    Failed(String),
}

/// One visible row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    pub depth: usize,
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub expanded: bool,
    /// An open folder whose listing hasn't arrived yet.
    pub loading: bool,
    /// Why an open folder couldn't be listed.
    pub error: Option<String>,
}

/// The tree pane's state.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    root: Option<PathBuf>,
    listings: HashMap<PathBuf, Listing>,
    expanded: HashSet<PathBuf>,
    /// The selected entry, by path, so it survives a refresh.
    selected: Option<PathBuf>,
    scroll: usize,
}

impl Tree {
    /// A tree rooted at `root`, selecting `select` once it shows. Returns the
    /// tree and the folder to list first.
    pub fn new(root: PathBuf, select: Option<PathBuf>) -> (Self, PathBuf) {
        let mut listings = HashMap::new();
        listings.insert(root.clone(), Listing::Loading);
        let tree = Self {
            root: Some(root.clone()),
            listings,
            expanded: HashSet::new(),
            selected: select,
            scroll: 0,
        };
        (tree, root)
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Whether the root's listing hasn't arrived yet.
    pub fn is_loading(&self) -> bool {
        self.root
            .as_ref()
            .is_some_and(|r| self.listings.get(r) == Some(&Listing::Loading))
    }

    /// Why the root couldn't be listed, if it couldn't.
    pub fn root_error(&self) -> Option<&str> {
        match self.listings.get(self.root.as_ref()?)? {
            Listing::Failed(e) => Some(e),
            _ => None,
        }
    }

    /// A listing arrived. Returns true if it was for a folder the tree knows.
    /// A listing arrived. Returns `None` for a folder the tree doesn't know;
    /// otherwise the folders it forgot because they went away (to stop
    /// watching them).
    pub fn listed(
        &mut self,
        dir: &Path,
        result: Result<Vec<Entry>, String>,
    ) -> Option<Vec<PathBuf>> {
        let slot = self.listings.get_mut(dir)?;
        *slot = match result {
            Ok(entries) => Listing::Loaded(entries),
            Err(e) => Listing::Failed(e),
        };
        // Entries that went away take their open state and listings with them.
        let mut forgotten = Vec::new();
        if let Listing::Loaded(entries) = &self.listings[dir] {
            let present: HashSet<&Path> = entries.iter().map(|e| e.path.as_path()).collect();
            let gone: Vec<PathBuf> = self
                .listings
                .keys()
                .filter(|p| p.parent() == Some(dir) && !present.contains(p.as_path()))
                .cloned()
                .collect();
            for p in gone {
                forgotten.extend(self.forget(&p));
            }
        }
        Some(forgotten)
    }

    /// Folders among `changed` (or the folders holding them) that the tree
    /// has asked for and should be listed again. A folder still loading
    /// counts: its listing may have been read before the change.
    pub fn stale(&self, changed: &[PathBuf]) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        for path in changed {
            let candidates = [Some(path.as_path()), path.parent()];
            for dir in candidates.into_iter().flatten() {
                if self.listings.contains_key(dir) && !dirs.iter().any(|d| d == dir) {
                    dirs.push(dir.to_path_buf());
                }
            }
        }
        dirs
    }

    /// Drops a folder (and everything under it) from the tree's state.
    /// Returns the folders whose listings it dropped.
    fn forget(&mut self, dir: &Path) -> Vec<PathBuf> {
        let dropped: Vec<PathBuf> = self
            .listings
            .keys()
            .filter(|p| p.starts_with(dir))
            .cloned()
            .collect();
        self.listings.retain(|p, _| !p.starts_with(dir));
        self.expanded.retain(|p| !p.starts_with(dir));
        dropped
    }

    /// Every folder the tree has asked to list (the root first), to list
    /// again when what the tree shows changes.
    pub fn listed_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = self.listings.keys().cloned().collect();
        dirs.sort();
        dirs
    }

    /// The visible rows, in order.
    pub fn rows(&self) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        let Some(root) = &self.root else {
            return rows;
        };
        // Iterative depth-first walk: deep trees can't overflow the stack.
        let mut stack: Vec<(&Entry, usize)> = Vec::new();
        if let Some(Listing::Loaded(entries)) = self.listings.get(root) {
            stack.extend(entries.iter().rev().map(|e| (e, 0)));
        }
        while let Some((entry, depth)) = stack.pop() {
            let expanded = entry.is_dir && self.expanded.contains(&entry.path);
            let listing = expanded.then(|| self.listings.get(&entry.path)).flatten();
            rows.push(TreeRow {
                depth,
                name: entry.name.clone(),
                path: entry.path.clone(),
                is_dir: entry.is_dir,
                expanded,
                loading: matches!(listing, Some(Listing::Loading)),
                error: match listing {
                    Some(Listing::Failed(e)) => Some(e.clone()),
                    _ => None,
                },
            });
            if let Some(Listing::Loaded(children)) = listing {
                stack.extend(children.iter().rev().map(|c| (c, depth + 1)));
            }
        }
        rows
    }

    /// The selected row's index, if the selection shows. With no selection,
    /// or one that went away, the first row.
    pub fn selected_index(&self, rows: &[TreeRow]) -> Option<usize> {
        if rows.is_empty() {
            return None;
        }
        let found = self
            .selected
            .as_ref()
            .and_then(|s| rows.iter().position(|r| &r.path == s));
        Some(found.unwrap_or(0))
    }

    pub fn selected(&self) -> Option<&Path> {
        self.selected.as_deref()
    }

    /// Moves the selection by `delta` rows, stopping at the ends.
    pub fn move_by(&mut self, delta: isize) {
        let rows = self.rows();
        let Some(i) = self.selected_index(&rows) else {
            return;
        };
        let j = i.saturating_add_signed(delta).min(rows.len() - 1);
        self.selected = Some(rows[j].path.clone());
    }

    /// Selects the first or last row.
    pub fn move_to_end(&mut self, last: bool) {
        let rows = self.rows();
        let row = if last { rows.last() } else { rows.first() };
        if let Some(row) = row {
            self.selected = Some(row.path.clone());
        }
    }

    /// Opens a closed folder. Returns the folder to list if it hasn't been.
    pub fn expand(&mut self, dir: &Path) -> Option<PathBuf> {
        self.expanded.insert(dir.to_path_buf());
        // A failed listing is tried again: the folder may be readable now.
        if matches!(
            self.listings.get(dir),
            Some(Listing::Loaded(_) | Listing::Loading)
        ) {
            None
        } else {
            self.listings.insert(dir.to_path_buf(), Listing::Loading);
            Some(dir.to_path_buf())
        }
    }

    /// Closes an open folder.
    pub fn collapse(&mut self, dir: &Path) {
        self.expanded.remove(dir);
    }

    pub fn is_expanded(&self, dir: &Path) -> bool {
        self.expanded.contains(dir)
    }

    /// Selects a path's parent folder, if that folder is a row.
    pub fn select_parent(&mut self) -> bool {
        let rows = self.rows();
        let Some(i) = self.selected_index(&rows) else {
            return false;
        };
        let Some(parent) = rows[i].path.parent() else {
            return false;
        };
        if rows.iter().any(|r| r.path == parent) {
            self.selected = Some(parent.to_path_buf());
            true
        } else {
            false
        }
    }

    /// The first row shown, keeping the selection on screen in a pane
    /// `height` rows tall.
    pub fn scroll_for(&mut self, height: usize) -> usize {
        let rows = self.rows();
        let height = height.max(1);
        if let Some(i) = self.selected_index(&rows) {
            if i < self.scroll {
                self.scroll = i;
            } else if i >= self.scroll + height {
                self.scroll = i + 1 - height;
            }
        }
        self.scroll = self.scroll.min(rows.len().saturating_sub(height));
        self.scroll
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, is_dir: bool) -> Entry {
        let path = PathBuf::from(path);
        Entry {
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            path,
            is_dir,
        }
    }

    fn names(tree: &Tree) -> Vec<String> {
        tree.rows()
            .iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth), r.name))
            .collect()
    }

    fn sample() -> Tree {
        let (mut t, root) = Tree::new("/p".into(), None);
        assert!(t.is_loading());
        t.listed(
            &root,
            Ok(vec![entry("/p/docs", true), entry("/p/a.md", false)]),
        );
        t
    }

    #[test]
    fn rows_nest_open_folders() {
        let mut t = sample();
        assert_eq!(names(&t), vec!["docs", "a.md"]);
        assert_eq!(t.expand(Path::new("/p/docs")), Some("/p/docs".into()));
        assert!(t.rows()[0].loading);
        t.listed(Path::new("/p/docs"), Ok(vec![entry("/p/docs/x.md", false)]));
        assert_eq!(names(&t), vec!["docs", "  x.md", "a.md"]);
        // Closing keeps the listing: opening again needs no new request.
        t.collapse(Path::new("/p/docs"));
        assert_eq!(names(&t), vec!["docs", "a.md"]);
        assert_eq!(t.expand(Path::new("/p/docs")), None);
    }

    #[test]
    fn selection_moves_and_survives_refresh() {
        let mut t = sample();
        assert_eq!(t.selected_index(&t.rows()), Some(0));
        t.move_by(5);
        assert_eq!(t.selected(), Some(Path::new("/p/a.md")));
        // A refresh that adds an entry above keeps the same file selected.
        t.listed(
            Path::new("/p"),
            Ok(vec![
                entry("/p/docs", true),
                entry("/p/0.md", false),
                entry("/p/a.md", false),
            ]),
        );
        assert_eq!(t.selected_index(&t.rows()), Some(2));
        t.move_to_end(false);
        assert_eq!(t.selected(), Some(Path::new("/p/docs")));
    }

    #[test]
    fn failed_listings_show_their_error() {
        let (mut t, root) = Tree::new("/p".into(), None);
        t.listed(&root, Err("permission denied".into()));
        assert_eq!(t.root_error(), Some("permission denied"));
        let mut t = sample();
        t.expand(Path::new("/p/docs"));
        t.listed(Path::new("/p/docs"), Err("nope".into()));
        assert_eq!(t.rows()[0].error.as_deref(), Some("nope"));
    }

    #[test]
    fn removed_folders_are_forgotten() {
        let mut t = sample();
        t.expand(Path::new("/p/docs"));
        t.listed(Path::new("/p/docs"), Ok(vec![]));
        t.listed(Path::new("/p"), Ok(vec![entry("/p/a.md", false)]));
        assert!(!t.is_expanded(Path::new("/p/docs")));
        assert_eq!(t.stale(&["/p/docs/x.md".into()]), Vec::<PathBuf>::new());
    }

    #[test]
    fn stale_folders_from_changes() {
        let mut t = sample();
        t.expand(Path::new("/p/docs"));
        t.listed(Path::new("/p/docs"), Ok(vec![]));
        assert_eq!(
            t.stale(&["/p/docs/new.md".into(), "/p/docs".into(), "/q/x".into()]),
            vec![PathBuf::from("/p/docs"), PathBuf::from("/p")]
        );
    }

    #[test]
    fn parent_and_scroll() {
        let mut t = sample();
        t.expand(Path::new("/p/docs"));
        t.listed(
            Path::new("/p/docs"),
            Ok((0..10)
                .map(|i| entry(&format!("/p/docs/{i}.md"), false))
                .collect()),
        );
        t.move_by(8);
        assert_eq!(t.scroll_for(4), 5);
        assert!(t.select_parent());
        assert_eq!(t.selected(), Some(Path::new("/p/docs")));
        assert_eq!(t.scroll_for(4), 0);
        assert!(!t.select_parent(), "the root isn't a row");
    }

    #[test]
    fn changes_during_a_listing_ask_again() {
        let mut t = sample();
        t.expand(Path::new("/p/docs"));
        assert_eq!(
            t.stale(&["/p/docs/new.md".into()]),
            vec![PathBuf::from("/p/docs")]
        );
    }

    #[test]
    fn listings_for_unknown_folders_are_ignored() {
        let mut t = sample();
        assert!(t.listed(Path::new("/elsewhere"), Ok(vec![])).is_none());
    }

    #[test]
    fn failed_folders_are_tried_again_on_open() {
        let mut t = sample();
        t.expand(Path::new("/p/docs"));
        t.listed(Path::new("/p/docs"), Err("EIO".into()));
        t.collapse(Path::new("/p/docs"));
        assert_eq!(t.expand(Path::new("/p/docs")), Some("/p/docs".into()));
    }

    #[test]
    fn forgotten_folders_are_reported() {
        let mut t = sample();
        t.expand(Path::new("/p/docs"));
        t.listed(Path::new("/p/docs"), Ok(vec![entry("/p/docs/sub", true)]));
        t.expand(Path::new("/p/docs/sub"));
        let mut gone = t
            .listed(Path::new("/p"), Ok(vec![entry("/p/a.md", false)]))
            .unwrap();
        gone.sort();
        assert_eq!(
            gone,
            vec![PathBuf::from("/p/docs"), PathBuf::from("/p/docs/sub")]
        );
    }
}
