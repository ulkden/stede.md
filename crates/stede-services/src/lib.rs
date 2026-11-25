// SPDX-License-Identifier: GPL-3.0-or-later
//! Background services for STEDE.md: folder listings and file watching
//! (polling on `/mnt/*` paths in WSL, where inotify doesn't fire). Later, the
//! linter hub and external commands.
//!
//! Work runs on a tokio runtime (`spawn_blocking` for file system calls,
//! watching and the change debouncer included) and reports back through a
//! sink the front end provides, so nothing here blocks the input loop. No
//! UI code here.

mod files;

pub use files::{MAX_FILE_BYTES, list_dir, open_file};

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use notify::{PollWatcher, RecommendedWatcher, RecursiveMode, Watcher};
use stede_core::config::read_layers;
use stede_core::io::{Request, ServiceEvent, TreeFilter};

/// Where results go. Called from service threads. It may block while the
/// front end's queue is full; once the front end is gone it must return.
pub type Sink = Arc<dyn Fn(ServiceEvent) + Send + Sync>;

/// How long changes collect before they're reported as one event, so a
/// burst (a `git checkout`, a build) costs one re-list per folder.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Folders watched at most for the whole tree (`Request::WatchTree`), well
/// under the usual inotify limits (8192 on older kernels).
const MAX_DEEP_WATCHES: usize = 4096;

/// How often the polling watcher (WSL `/mnt/*` paths) checks for changes.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// The running services.
pub struct Services {
    /// Taken on drop, to shut down without waiting for stuck work.
    runtime: Option<tokio::runtime::Runtime>,
    shared: Arc<Shared>,
}

/// State the runtime's tasks and the debouncer share.
struct Shared {
    sink: Sink,
    watch: Mutex<Watch>,
    /// Config files to watch, in layer order.
    config_files: Mutex<Vec<PathBuf>>,
    /// Folders being listed, and whether another listing was asked for
    /// meanwhile. One listing per folder at a time keeps results in order.
    inflight: Mutex<HashMap<PathBuf, bool>>,
    /// What listings show; set by the core.
    filter: Mutex<TreeFilter>,
    /// Folders the tree has asked to stop watching, newest request winning:
    /// `handle` updates it in request order (a listing clears it, `Unwatch`
    /// sets it), and the watch and unwatch tasks check it under the watch
    /// lock, so whichever runs last can't undo a newer request.
    tree_unwanted: Mutex<HashSet<PathBuf>>,
    /// Files being opened, and the flavor of a request that came in during
    /// the read (`Some`): the file is read again with the newest flavor.
    opening: Mutex<HashMap<PathBuf, Option<Option<String>>>>,
    /// The whole-tree watch. Lock order: `deep`, then `watch`, then
    /// `tree_unwanted`; nothing takes them the other way round.
    deep: Mutex<Deep>,
    /// The cap on whole-tree watches (a field so tests can lower it).
    deep_limit: usize,
}

/// The whole-tree watch's state.
#[derive(Default)]
struct Deep {
    root: Option<PathBuf>,
    /// Bumped by each full walk: a walk that finishes after a newer one
    /// began drops its result.
    generation: u64,
    /// The folders it watches.
    dirs: HashSet<PathBuf>,
    /// Whether the cap has been reported (once a session).
    warned: bool,
}

/// File watchers, made on first use.
struct Watch {
    /// `None` once the services shut down: with it and the watchers gone,
    /// the debouncer's channel closes and it ends.
    events: Option<mpsc::Sender<notify::Result<notify::Event>>>,
    native: Option<RecommendedWatcher>,
    poll: Option<PollWatcher>,
    /// Each watched folder, as given, to its resolved path. Watchers
    /// (FSEvents on macOS) may report resolved paths, such as
    /// `/private/tmp/...` for `/tmp/...`; events are mapped back with this.
    resolved: HashMap<PathBuf, PathBuf>,
    /// Who wants each folder watched. The tree and the config watcher can
    /// share one (`.stede/` open in the tree): the OS watch goes only when
    /// neither does.
    owners: HashMap<PathBuf, Owners>,
}

#[derive(Clone, Copy, Debug, Default)]
struct Owners {
    tree: bool,
    config: bool,
    deep: bool,
}

impl Owners {
    fn set(&mut self, owner: Owner, on: bool) {
        match owner {
            Owner::Tree => self.tree = on,
            Owner::Config => self.config = on,
            Owner::Deep => self.deep = on,
        }
    }

    fn any(self) -> bool {
        self.tree || self.config || self.deep
    }
}

/// Who asks for a watch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    /// A folder the tree lists.
    Tree,
    /// A folder holding a config file.
    Config,
    /// The whole-tree watch.
    Deep,
}

impl Watch {
    /// Adds the OS watch for `dir` (already resolved) if it has none, and
    /// records `owner` as wanting it.
    fn add(&mut self, dir: &Path, resolved: PathBuf, poll: bool, owner: Owner) {
        self.add_mode(dir, resolved, poll, owner, RecursiveMode::NonRecursive);
    }

    fn add_mode(
        &mut self,
        dir: &Path,
        resolved: PathBuf,
        poll: bool,
        owner: Owner,
        mode: RecursiveMode,
    ) {
        let Some(events) = self.events.clone() else {
            return;
        };
        self.owners
            .entry(dir.to_path_buf())
            .or_default()
            .set(owner, true);
        self.resolved.insert(dir.to_path_buf(), resolved);
        // A folder that can't be watched still lists; it just won't refresh
        // on its own until it's listed (and watched) again.
        if poll {
            if self.poll.is_none() {
                let config = notify::Config::default().with_poll_interval(POLL_INTERVAL);
                self.poll = PollWatcher::new(events, config).ok();
            }
            if let Some(w) = self.poll.as_mut() {
                let _ = w.watch(dir, mode);
            }
        } else {
            if self.native.is_none() {
                self.native = notify::recommended_watcher(events).ok();
            }
            if let Some(w) = self.native.as_mut() {
                let _ = w.watch(dir, mode);
            }
        }
    }

    /// `owner` no longer wants `dir` watched; the OS watch goes once no one
    /// does.
    fn drop_owner(&mut self, dir: &Path, owner: Owner) {
        if let Some(owners) = self.owners.get_mut(dir) {
            owners.set(owner, false);
            if owners.any() {
                return;
            }
        }
        self.owners.remove(dir);
        self.resolved.remove(dir);
        if let Some(w) = self.native.as_mut() {
            let _ = w.unwatch(dir);
        }
        if let Some(w) = self.poll.as_mut() {
            let _ = w.unwatch(dir);
        }
    }
}

impl Services {
    /// Starts the runtime and the change debouncer.
    pub fn start(sink: Sink) -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("stede-services")
            .build()?;
        let (events, raw) = mpsc::channel();
        let shared = Arc::new(Shared {
            sink,
            watch: Mutex::new(Watch {
                events: Some(events),
                native: None,
                poll: None,
                resolved: HashMap::new(),
                owners: HashMap::new(),
            }),
            config_files: Mutex::new(Vec::new()),
            inflight: Mutex::new(HashMap::new()),
            filter: Mutex::new(TreeFilter::default()),
            tree_unwanted: Mutex::new(HashSet::new()),
            opening: Mutex::new(HashMap::new()),
            deep: Mutex::new(Deep::default()),
            deep_limit: MAX_DEEP_WATCHES,
        });
        {
            // A long-running blocking task: the debouncer waits on a channel.
            let shared = Arc::clone(&shared);
            runtime.spawn_blocking(move || debounce(&raw, &shared));
        }
        Ok(Self {
            runtime: Some(runtime),
            shared,
        })
    }

    fn runtime(&self) -> &tokio::runtime::Runtime {
        self.runtime.as_ref().expect("the runtime lives until drop")
    }

    /// Takes a request from the core.
    pub fn handle(&self, request: Request) {
        match request {
            Request::ListDir(dir) => {
                self.shared
                    .tree_unwanted
                    .lock()
                    .expect("unwanted lock")
                    .remove(&dir);
                {
                    let mut inflight = self.shared.inflight.lock().expect("inflight lock");
                    if let Some(again) = inflight.get_mut(&dir) {
                        // Listed again as soon as the current one is done.
                        *again = true;
                        return;
                    }
                    inflight.insert(dir.clone(), false);
                }
                let shared = Arc::clone(&self.shared);
                self.runtime()
                    .spawn_blocking(move || shared.list_until_settled(dir));
            }
            Request::TreeFilter(filter) => {
                *self.shared.filter.lock().expect("filter lock") = filter;
                // What counts as hidden or ignored changed: walk again.
                if self.shared.deep.lock().expect("deep lock").root.is_some() {
                    self.walk_tree();
                }
            }
            Request::WatchTree(root) => {
                self.shared.deep.lock().expect("deep lock").root = Some(root);
                self.walk_tree();
            }
            Request::Unwatch(dir) => {
                self.shared
                    .tree_unwanted
                    .lock()
                    .expect("unwanted lock")
                    .insert(dir.clone());
                let shared = Arc::clone(&self.shared);
                self.runtime()
                    .spawn_blocking(move || shared.unwatch_dir(&dir));
            }
            Request::OpenFile { path, flavor } => {
                // One open per file at a time, like listings, so the newest
                // read is the one reported last, with the newest flavor.
                {
                    let mut opening = self.shared.opening.lock().expect("opening lock");
                    if let Some(again) = opening.get_mut(&path) {
                        *again = Some(flavor);
                        return;
                    }
                    opening.insert(path.clone(), None);
                }
                let shared = Arc::clone(&self.shared);
                self.runtime().spawn_blocking(move || {
                    let mut flavor = flavor;
                    loop {
                        let result = guarded(|| open_file(&path, flavor.as_deref()));
                        (shared.sink)(ServiceEvent::Opened {
                            path: path.clone(),
                            result,
                        });
                        let mut opening = shared.opening.lock().expect("opening lock");
                        match opening.get_mut(&path).and_then(Option::take) {
                            Some(newer) => flavor = newer,
                            None => {
                                opening.remove(&path);
                                return;
                            }
                        }
                    }
                });
            }
        }
    }

    /// Starts a full walk of the whole-tree watch, newest walk winning.
    fn walk_tree(&self) {
        let generation = {
            let mut deep = self.shared.deep.lock().expect("deep lock");
            deep.generation += 1;
            deep.generation
        };
        let shared = Arc::clone(&self.shared);
        self.runtime()
            .spawn_blocking(move || shared.deep_walk(generation));
    }

    /// Watches config files, reporting `ConfigChanged` (with every layer read
    /// again) when one changes. Each file's nearest existing folder is
    /// watched, so a file, or a folder for it, created later still counts.
    pub fn watch_config(&self, files: Vec<PathBuf>) {
        *self.shared.config_files.lock().expect("config lock") = files;
        let shared = Arc::clone(&self.shared);
        self.runtime()
            .spawn_blocking(move || shared.watch_config_folders());
    }
}

impl Drop for Services {
    fn drop(&mut self) {
        // Don't wait for listings stuck on a slow or hung mount: quitting
        // must not hang after the terminal is restored.
        // Stop watching: dropping the watchers and the last sender closes
        // the debouncer's channel, so it ends instead of calling a sink whose
        // front end is gone.
        if let Ok(mut watch) = self.shared.watch.lock() {
            watch.native = None;
            watch.poll = None;
            watch.events = None;
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl Shared {
    /// Lists a folder, again and again while more listings were asked for
    /// during one, then reports each in order.
    fn list_until_settled(&self, dir: PathBuf) {
        loop {
            // Watch first, so a change during the listing isn't missed; and
            // every time, so a folder deleted and made again is watched again.
            self.watch_dir(&dir, Owner::Tree);
            // Read each time: a listing asked for again after the filter
            // changed shows the new one.
            let filter = *self.filter.lock().expect("filter lock");
            let result = guarded(|| list_dir(&dir, filter));
            (self.sink)(ServiceEvent::Listed {
                dir: dir.clone(),
                result,
            });
            let mut inflight = self.inflight.lock().expect("inflight lock");
            if inflight.get(&dir) == Some(&true) {
                inflight.insert(dir.clone(), false);
            } else {
                inflight.remove(&dir);
                return;
            }
        }
    }

    fn watch_config_folders(&self) {
        let files = self.config_files.lock().expect("config lock").clone();
        for file in &files {
            if let Some(dir) = file.ancestors().skip(1).find(|d| d.is_dir()) {
                self.watch_dir(dir, Owner::Config);
            }
        }
    }

    fn watch_dir(&self, dir: &Path, owner: Owner) {
        let resolved = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        // The resolved path decides: a symlink into `/mnt/c` needs polling.
        let poll = needs_polling(&resolved);
        let mut watch = self.watch.lock().expect("watch lock");
        // An `Unwatch` the tree sent after this listing's request wins.
        if owner == Owner::Tree
            && self
                .tree_unwanted
                .lock()
                .expect("unwanted lock")
                .contains(dir)
        {
            return;
        }
        watch.add(dir, resolved, poll, owner);
    }

    /// Walks the tree's root for every folder the tree would show and
    /// watches them, replacing the last walk's set. Skipped where watching
    /// means polling: a poll of a whole tree costs a scan every interval.
    fn deep_walk(&self, generation: u64) {
        let Some(root) = self.deep.lock().expect("deep lock").root.clone() else {
            return;
        };
        let resolved_root = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        if needs_polling(&resolved_root) {
            return;
        }
        // FSEvents watches a tree natively, and its watcher restarts its
        // stream on every added path: one recursive watch beats thousands.
        // What it reports from ignored folders the core drops (it only
        // re-lists folders the tree shows).
        if cfg!(target_os = "macos") {
            let mut deep = self.deep.lock().expect("deep lock");
            if deep.generation != generation || deep.dirs.contains(&root) {
                return;
            }
            let mut watch = self.watch.lock().expect("watch lock");
            watch.add_mode(
                &root,
                resolved_root,
                false,
                Owner::Deep,
                RecursiveMode::Recursive,
            );
            drop(watch);
            deep.dirs = HashSet::from([root]);
            return;
        }
        let filter = *self.filter.lock().expect("filter lock");
        let (found, capped) = walk_dirs(&root, filter, self.deep_limit);
        // Resolve before taking locks: it touches the file system.
        let found: Vec<(PathBuf, PathBuf)> = found
            .into_iter()
            .map(|d| {
                let r = std::fs::canonicalize(&d).unwrap_or_else(|_| d.clone());
                (d, r)
            })
            .collect();
        let warn = {
            let mut deep = self.deep.lock().expect("deep lock");
            if deep.generation != generation {
                return;
            }
            let wanted: HashSet<PathBuf> = found.iter().map(|(d, _)| d.clone()).collect();
            let mut watch = self.watch.lock().expect("watch lock");
            for gone in deep.dirs.difference(&wanted) {
                watch.drop_owner(gone, Owner::Deep);
            }
            for (dir, resolved) in found {
                if !deep.dirs.contains(&dir) {
                    watch.add(&dir, resolved, false, Owner::Deep);
                }
            }
            drop(watch);
            deep.dirs = wanted;
            let warn = capped && !deep.warned;
            deep.warned |= capped;
            warn
        };
        if warn {
            self.notice_cap();
        }
    }

    /// Folders among `paths` that appeared under the watched tree (and that
    /// the tree would show) get watched, with what's inside them; ones that
    /// went away are dropped.
    fn deep_follow(&self, paths: &[PathBuf]) {
        // The recursive watch on macOS already covers new folders.
        if cfg!(target_os = "macos") {
            return;
        }
        let (root, generation) = {
            let deep = self.deep.lock().expect("deep lock");
            match &deep.root {
                Some(root) => (root.clone(), deep.generation),
                None => return,
            }
        };
        let filter = *self.filter.lock().expect("filter lock");
        let mut found: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut gone: Vec<PathBuf> = Vec::new();
        let mut capped = false;
        for path in paths.iter().filter(|p| p.starts_with(&root) && *p != &root) {
            if !path.exists() {
                gone.push(path.clone());
                continue;
            }
            if !path.is_dir() || self.deep.lock().expect("deep lock").dirs.contains(path) {
                continue;
            }
            // Only what the tree would show: its parent's listing decides
            // (hidden, ignored), as for any other entry.
            let Some(parent) = path.parent() else {
                continue;
            };
            let shown = list_dir(parent, filter).is_ok_and(|e| e.iter().any(|e| &e.path == path));
            if !shown {
                continue;
            }
            let (dirs, over) = walk_dirs(path, filter, self.deep_limit);
            capped |= over;
            found.extend(dirs.into_iter().map(|d| {
                let r = std::fs::canonicalize(&d).unwrap_or_else(|_| d.clone());
                (d, r)
            }));
        }
        if found.is_empty() && gone.is_empty() {
            return;
        }
        let warn = {
            let mut deep = self.deep.lock().expect("deep lock");
            if deep.generation != generation {
                return;
            }
            let mut watch = self.watch.lock().expect("watch lock");
            let removed: Vec<PathBuf> = deep
                .dirs
                .iter()
                .filter(|d| gone.iter().any(|g| d.starts_with(g)))
                .cloned()
                .collect();
            for dir in removed {
                watch.drop_owner(&dir, Owner::Deep);
                deep.dirs.remove(&dir);
            }
            for (dir, resolved) in found {
                if deep.dirs.len() >= self.deep_limit {
                    capped = true;
                    break;
                }
                if deep.dirs.insert(dir.clone()) {
                    watch.add(&dir, resolved, false, Owner::Deep);
                }
            }
            drop(watch);
            let warn = capped && !deep.warned;
            deep.warned |= capped;
            warn
        };
        if warn {
            self.notice_cap();
        }
    }

    fn notice_cap(&self) {
        (self.sink)(ServiceEvent::Notice(format!(
            "this folder has over {} folders: only the first {} and the ones you open are watched for changes",
            self.deep_limit, self.deep_limit
        )));
    }

    /// Stops the tree's watch on a folder; the OS watch goes unless the
    /// config watcher or the whole-tree watch still wants it.
    fn unwatch_dir(&self, dir: &Path) {
        let mut watch = self.watch.lock().expect("watch lock");
        // A listing the tree asked for after this `Unwatch` wants it watched.
        if !self
            .tree_unwanted
            .lock()
            .expect("unwanted lock")
            .contains(dir)
        {
            return;
        }
        watch.drop_owner(dir, Owner::Tree);
    }

    /// A watcher's path under every name the tree knows it by: as reported,
    /// and with the resolved prefix of each folder watched through a link
    /// (longest first) mapped back to the link. One folder can be watched
    /// both ways, so both names are reported.
    fn as_given(&self, path: PathBuf) -> Vec<PathBuf> {
        let watch = self.watch.lock().expect("watch lock");
        let mut mapped: Vec<(usize, PathBuf)> = watch
            .resolved
            .iter()
            .filter(|(given, resolved)| given != resolved && path.starts_with(resolved))
            .filter_map(|(given, resolved)| {
                let rest = path.strip_prefix(resolved).ok()?;
                Some((resolved.as_os_str().len(), given.join(rest)))
            })
            .collect();
        mapped.sort_by_key(|m| std::cmp::Reverse(m.0));
        let mut names = vec![path];
        names.extend(mapped.into_iter().map(|(_, p)| p));
        names
    }
}

/// Runs a read, turning a panic (a parser bug on odd input, say) into an
/// error, so its in-flight entry is cleared and the path can be tried again.
fn guarded<T>(read: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(read)).unwrap_or_else(|_| {
        Err("an internal error stopped the read (a bug; please report it)".into())
    })
}

/// Collects raw watcher events and reports them in batches: the paths that
/// changed, and the config layers again if a config file (or a folder on
/// the way to one) did.
fn debounce(raw: &mpsc::Receiver<notify::Result<notify::Event>>, shared: &Shared) {
    // Ends when every sender (the watchers and `Services`) is gone.
    while let Ok(first) = raw.recv() {
        let mut paths: Vec<PathBuf> = Vec::new();
        let mut take = |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            // Reading a folder (our own listings do) is an access, not a
            // change; counting it would make every listing ask for another.
            if matches!(event.kind, notify::EventKind::Access(_)) {
                return;
            }
            for path in event.paths.into_iter().flat_map(|p| shared.as_given(p)) {
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
        };
        take(first);
        let until = Instant::now() + DEBOUNCE;
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match raw.recv_timeout(left) {
                Ok(event) => take(event),
                Err(_) => break,
            }
        }
        if paths.is_empty() {
            continue;
        }
        let files = shared.config_files.lock().expect("config lock").clone();
        let config_changed = paths
            .iter()
            .any(|p| files.iter().any(|f| f == p || f.starts_with(p)));
        // New folders under the whole-tree watch get watched before the
        // core hears of them, so what happens inside them next is seen.
        shared.deep_follow(&paths);
        (shared.sink)(ServiceEvent::Changed(paths));
        if config_changed {
            // Folders that now exist get watched, then every layer is read
            // again here, off the input loop.
            shared.watch_config_folders();
            let (layers, errors) = read_layers(&files);
            (shared.sink)(ServiceEvent::ConfigChanged { layers, errors });
        }
    }
}

/// Whether a folder needs the polling watcher: under WSL, inotify doesn't
/// fire for Windows drives mounted at `/mnt/*`.
/// Every folder under `root` (itself included) that the tree would show by
/// `filter`, stopping at `limit`. Returns them and whether it stopped short.
fn walk_dirs(root: &Path, filter: TreeFilter, limit: usize) -> (Vec<PathBuf>, bool) {
    let walker = files::tree_walker(root, filter).build();
    let mut dirs = Vec::new();
    for item in walker {
        let Ok(item) = item else { continue };
        if !item.file_type().is_some_and(|t| t.is_dir()) {
            continue;
        }
        if dirs.len() == limit {
            return (dirs, true);
        }
        dirs.push(item.into_path());
    }
    (dirs, false)
}

fn needs_polling(dir: &Path) -> bool {
    dir.starts_with("/mnt/") && is_wsl()
}

fn is_wsl() -> bool {
    static WSL: OnceLock<bool> = OnceLock::new();
    *WSL.get_or_init(|| {
        std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .is_ok_and(|release| release.to_ascii_lowercase().contains("microsoft"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("stede-services-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    fn collecting() -> (Sink, mpsc::Receiver<ServiceEvent>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let sink: Sink = Arc::new(move |e| {
            let _ = tx.lock().unwrap().send(e);
        });
        (sink, rx)
    }

    fn wait_for(rx: &mpsc::Receiver<ServiceEvent>, pred: impl Fn(&ServiceEvent) -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(10);
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(e) if pred(&e) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
        false
    }

    #[test]
    fn lists_then_reports_changes() {
        let dir = temp("watch");
        std::fs::write(dir.join("a.md"), "a").unwrap();
        let (sink, rx) = collecting();
        let services = Services::start(sink).unwrap();
        services.handle(Request::ListDir(dir.clone()));
        assert!(wait_for(&rx, |e| matches!(
            e,
            ServiceEvent::Listed { result: Ok(entries), .. } if entries.len() == 1
        )));
        std::fs::write(dir.join("b.md"), "b").unwrap();
        assert!(
            wait_for(
                &rx,
                |e| matches!(e, ServiceEvent::Changed(paths) if paths.iter().any(|p| p.ends_with("b.md")))
            ),
            "a new file is reported"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn listings_follow_the_tree_filter() {
        let dir = temp("filter");
        std::fs::write(dir.join("a.md"), "a").unwrap();
        std::fs::write(dir.join(".dot.md"), "d").unwrap();
        let (sink, rx) = collecting();
        let services = Services::start(sink).unwrap();
        services.handle(Request::ListDir(dir.clone()));
        assert!(wait_for(&rx, |e| matches!(
            e,
            ServiceEvent::Listed { result: Ok(entries), .. } if entries.len() == 1
        )));
        services.handle(Request::TreeFilter(TreeFilter {
            show_hidden: true,
            show_ignored: false,
        }));
        services.handle(Request::ListDir(dir.clone()));
        assert!(
            wait_for(&rx, |e| matches!(
                e,
                ServiceEvent::Listed { result: Ok(entries), .. } if entries.len() == 2
            )),
            "the dotfile shows once the filter says so"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn repeated_listings_settle_on_the_latest() {
        let dir = temp("burst");
        let (sink, rx) = collecting();
        let services = Services::start(sink).unwrap();
        for i in 0..5 {
            std::fs::write(dir.join(format!("{i}.md")), "x").unwrap();
            services.handle(Request::ListDir(dir.clone()));
        }
        // However the requests overlapped, the last listing reported sees all five.
        let mut last = None;
        while let Ok(e) = rx.recv_timeout(Duration::from_millis(1500)) {
            if let ServiceEvent::Listed {
                result: Ok(entries),
                ..
            } = e
            {
                last = Some(entries.len());
            }
        }
        assert_eq!(last, Some(5));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn recreated_folders_are_watched_again() {
        let dir = temp("recreate");
        let sub = dir.join("docs");
        std::fs::create_dir(&sub).unwrap();
        let (sink, rx) = collecting();
        let services = Services::start(sink).unwrap();
        services.handle(Request::ListDir(sub.clone()));
        assert!(wait_for(&rx, |e| matches!(e, ServiceEvent::Listed { .. })));
        std::fs::remove_dir_all(&sub).unwrap();
        std::fs::create_dir(&sub).unwrap();
        services.handle(Request::ListDir(sub.clone()));
        assert!(wait_for(&rx, |e| matches!(e, ServiceEvent::Listed { .. })));
        std::fs::write(sub.join("new.md"), "x").unwrap();
        assert!(wait_for(
            &rx,
            |e| matches!(e, ServiceEvent::Changed(paths) if paths.iter().any(|p| p.ends_with("new.md")))
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn config_changes_are_read_and_reported() {
        let dir = temp("config");
        // The folder for the config doesn't exist yet: creating it counts.
        let file = dir.join("stede").join("config.toml");
        let (sink, rx) = collecting();
        let services = Services::start(sink).unwrap();
        services.watch_config(vec![file.clone()]);
        std::thread::sleep(Duration::from_millis(200));
        std::fs::create_dir(dir.join("stede")).unwrap();
        // Creating the folder is reported (with no layers yet)...
        assert!(wait_for(&rx, |e| matches!(
            e,
            ServiceEvent::ConfigChanged { .. }
        )));
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(&file, "[ui]\n").unwrap();
        // ...and then the file inside it, with its text.
        assert!(wait_for(&rx, |e| matches!(
            e,
            ServiceEvent::ConfigChanged { layers, .. } if layers.len() == 1 && layers[0].text == "[ui]\n"
        )));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_tree_unwatching_a_config_folder_keeps_config_reloads() {
        let dir = temp("shared-watch");
        let stede = dir.join(".stede");
        std::fs::create_dir(&stede).unwrap();
        let file = stede.join("config.toml");
        std::fs::write(&file, "").unwrap();
        let (sink, rx) = collecting();
        let services = Services::start(sink).unwrap();
        services.watch_config(vec![file.clone()]);
        // The tree opens .stede/, then drops it (dotfiles hidden).
        services.handle(Request::ListDir(stede.clone()));
        assert!(wait_for(&rx, |e| matches!(e, ServiceEvent::Listed { .. })));
        services.handle(Request::Unwatch(stede.clone()));
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(&file, "[ui]\n").unwrap();
        assert!(
            wait_for(&rx, |e| matches!(
                e,
                ServiceEvent::ConfigChanged { layers, .. } if layers.iter().any(|l| l.text == "[ui]\n")
            )),
            "the config watcher still sees its folder"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_newest_tree_request_wins_however_the_tasks_run() {
        let dir = temp("order");
        let (sink, _rx) = collecting();
        let services = Services::start(sink).unwrap();
        let shared = &services.shared;
        let watched = |d: &Path| {
            shared
                .watch
                .lock()
                .unwrap()
                .owners
                .get(d)
                .is_some_and(|o| o.tree)
        };
        // Unwatch, then a listing: the unwatch task running late is a no-op.
        shared.watch_dir(&dir, Owner::Tree);
        shared.tree_unwanted.lock().unwrap().insert(dir.clone());
        shared.tree_unwanted.lock().unwrap().remove(&dir);
        shared.unwatch_dir(&dir);
        assert!(watched(&dir), "the newer listing keeps it watched");
        // A listing, then Unwatch: the listing's watch running late is a no-op.
        shared.tree_unwanted.lock().unwrap().insert(dir.clone());
        shared.unwatch_dir(&dir);
        shared.watch_dir(&dir, Owner::Tree);
        assert!(!watched(&dir), "the newer unwatch keeps it unwatched");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Waits up to 10 s for `cond`.
    fn eventually(cond: impl Fn() -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn deep_watched(services: &Services, dir: &Path) -> bool {
        services
            .shared
            .watch
            .lock()
            .unwrap()
            .owners
            .get(dir)
            .is_some_and(|o| o.deep)
    }

    #[test]
    fn the_whole_tree_is_watched_but_not_what_it_hides() {
        let dir = temp("deep");
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::create_dir_all(dir.join("skip/inner")).unwrap();
        std::fs::create_dir_all(dir.join(".dot")).unwrap();
        std::fs::write(dir.join(".gitignore"), "skip/\n").unwrap();
        let (sink, rx) = collecting();
        let services = Services::start(sink).unwrap();
        services.handle(Request::WatchTree(dir.clone()));
        // macOS watches the root recursively instead (see `deep_walk`).
        let mac = cfg!(target_os = "macos");
        if mac {
            assert!(eventually(|| deep_watched(&services, &dir)));
        } else {
            assert!(eventually(|| deep_watched(&services, &dir.join("a/b"))));
            assert!(!deep_watched(&services, &dir.join("skip")), "ignored");
            assert!(!deep_watched(&services, &dir.join("skip/inner")));
            assert!(!deep_watched(&services, &dir.join(".dot")), "hidden");
        }
        // A change two folders down, never opened in the tree, is seen.
        std::fs::write(dir.join("a/b/x.md"), "x").unwrap();
        assert!(wait_for(&rx, |e| matches!(
            e,
            ServiceEvent::Changed(paths) if paths.iter().any(|p| p.ends_with("a/b/x.md"))
        )));
        // A folder made later is followed, and what happens inside it seen.
        std::fs::create_dir(dir.join("new")).unwrap();
        if !mac {
            assert!(eventually(|| deep_watched(&services, &dir.join("new"))));
        } else {
            std::thread::sleep(Duration::from_millis(300));
        }
        std::fs::write(dir.join("new/y.md"), "y").unwrap();
        assert!(wait_for(&rx, |e| matches!(
            e,
            ServiceEvent::Changed(paths) if paths.iter().any(|p| p.ends_with("new/y.md"))
        )));
        // Showing dotfiles walks again, and the tree's unwatch leaves the
        // whole-tree watch alone.
        services.handle(Request::TreeFilter(TreeFilter {
            show_hidden: true,
            show_ignored: false,
        }));
        if mac {
            return std::fs::remove_dir_all(&dir).unwrap();
        }
        assert!(eventually(|| deep_watched(&services, &dir.join(".dot"))));
        services.handle(Request::ListDir(dir.join("a")));
        services.handle(Request::Unwatch(dir.join("a")));
        std::thread::sleep(Duration::from_millis(200));
        assert!(deep_watched(&services, &dir.join("a")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_walk_stops_at_its_cap() {
        let dir = temp("cap");
        for i in 0..5 {
            std::fs::create_dir(dir.join(format!("d{i}"))).unwrap();
        }
        let (dirs, capped) = walk_dirs(&dir, TreeFilter::default(), 3);
        assert_eq!(dirs.len(), 3);
        assert!(capped);
        let (dirs, capped) = walk_dirs(&dir, TreeFilter::default(), 100);
        assert_eq!(dirs.len(), 6, "the root and its five folders");
        assert!(!capped);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resolved_paths_map_back_to_the_watched_one() {
        let dir = temp("symlink");
        let link = dir.with_file_name(format!(
            "{}-link",
            dir.file_name().unwrap().to_string_lossy()
        ));
        let _ = std::fs::remove_file(&link);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        let (sink, _rx) = collecting();
        let services = Services::start(sink).unwrap();
        services.shared.watch_dir(&link, Owner::Tree);
        assert!(
            services
                .shared
                .as_given(dir.join("x.md"))
                .contains(&link.join("x.md"))
        );
        assert_eq!(
            services.shared.as_given(link.join("y.md")),
            vec![link.join("y.md")]
        );
        // Watched both directly and through the link: both names come back.
        services.shared.watch_dir(&dir, Owner::Tree);
        let names = services.shared.as_given(dir.join("x.md"));
        assert!(
            names.contains(&dir.join("x.md")) && names.contains(&link.join("x.md")),
            "{names:?}"
        );
        let _ = std::fs::remove_file(&link);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn listing_a_folder_is_not_a_change() {
        let dir = temp("quiet");
        std::fs::write(dir.join("a.md"), "a").unwrap();
        // FSEvents (macOS) has latency and can still deliver the write above
        // after watching starts; let it settle first.
        std::thread::sleep(Duration::from_millis(1000));
        let (sink, rx) = collecting();
        let services = Services::start(sink).unwrap();
        for _ in 0..3 {
            services.handle(Request::ListDir(dir.clone()));
            std::thread::sleep(Duration::from_millis(100));
        }
        // The regression: reading the folder (an access) reported it changed,
        // so every listing asked for another. Opening a file in it mustn't
        // either (that would reload it forever).
        services.handle(Request::OpenFile {
            path: dir.join("a.md"),
            flavor: None,
        });
        let mut changed = Vec::new();
        while let Ok(e) = rx.recv_timeout(Duration::from_millis(600)) {
            if let ServiceEvent::Changed(paths) = e {
                changed.extend(paths.into_iter().filter(|p| p.starts_with(&dir)));
            }
        }
        assert!(changed.is_empty(), "reads reported as changes: {changed:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dropping_stops_watching() {
        let dir = temp("drop");
        let (sink, rx) = collecting();
        let services = Services::start(sink).unwrap();
        services.handle(Request::ListDir(dir.clone()));
        assert!(wait_for(&rx, |e| matches!(e, ServiceEvent::Listed { .. })));
        drop(services);
        // inotify's thread stops shortly after its watcher is dropped.
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(dir.join("after.md"), "x").unwrap();
        // Nothing arrives, and the sender side closes (the sink is dropped).
        assert!(!matches!(
            rx.recv_timeout(Duration::from_millis(600)),
            Ok(ServiceEvent::Changed(_))
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_panicking_read_is_an_error_not_a_stuck_path() {
        let result: Result<(), String> = guarded(|| panic!("boom"));
        assert!(result.unwrap_err().contains("internal error"));
    }

    #[test]
    fn polling_only_for_wsl_windows_drives() {
        assert!(!needs_polling(Path::new("/home/me/notes")));
        assert_eq!(needs_polling(Path::new("/mnt/c/notes")), is_wsl());
    }
}
