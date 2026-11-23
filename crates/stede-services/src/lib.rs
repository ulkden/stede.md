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

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use notify::{PollWatcher, RecommendedWatcher, RecursiveMode, Watcher};
use stede_core::config::read_layers;
use stede_core::io::{Request, ServiceEvent};

/// Where results go. Called from service threads. It may block while the
/// front end's queue is full; once the front end is gone it must return.
pub type Sink = Arc<dyn Fn(ServiceEvent) + Send + Sync>;

/// How long changes collect before they're reported as one event, so a
/// burst (a `git checkout`, a build) costs one re-list per folder.
const DEBOUNCE: Duration = Duration::from_millis(150);

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
            }),
            config_files: Mutex::new(Vec::new()),
            inflight: Mutex::new(HashMap::new()),
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
            Request::Unwatch(dir) => {
                let shared = Arc::clone(&self.shared);
                self.runtime()
                    .spawn_blocking(move || shared.unwatch_dir(&dir));
            }
            Request::OpenFile { path, flavor } => {
                // One open per file at a time, like listings, so the newest
                // read is the one reported last.
                let key = PathBuf::from("\0open").join(&path);
                {
                    let mut inflight = self.shared.inflight.lock().expect("inflight lock");
                    if let Some(again) = inflight.get_mut(&key) {
                        *again = true;
                        return;
                    }
                    inflight.insert(key.clone(), false);
                }
                let shared = Arc::clone(&self.shared);
                self.runtime().spawn_blocking(move || {
                    loop {
                        let result = open_file(&path, flavor.as_deref());
                        (shared.sink)(ServiceEvent::Opened {
                            path: path.clone(),
                            result,
                        });
                        let mut inflight = shared.inflight.lock().expect("inflight lock");
                        if inflight.get(&key) == Some(&true) {
                            inflight.insert(key.clone(), false);
                        } else {
                            inflight.remove(&key);
                            return;
                        }
                    }
                });
            }
        }
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
            self.watch_dir(&dir);
            let result = list_dir(&dir);
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
                self.watch_dir(dir);
            }
        }
    }

    fn watch_dir(&self, dir: &Path) {
        let resolved = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        // The resolved path decides: a symlink into `/mnt/c` needs polling.
        let poll = needs_polling(&resolved);
        let mut watch = self.watch.lock().expect("watch lock");
        let Some(events) = watch.events.clone() else {
            return;
        };
        watch.resolved.insert(dir.to_path_buf(), resolved);
        // A folder that can't be watched still lists; it just won't refresh
        // on its own until it's listed (and watched) again.
        if poll {
            if watch.poll.is_none() {
                let config = notify::Config::default().with_poll_interval(POLL_INTERVAL);
                watch.poll = PollWatcher::new(events, config).ok();
            }
            if let Some(w) = watch.poll.as_mut() {
                let _ = w.watch(dir, RecursiveMode::NonRecursive);
            }
        } else {
            if watch.native.is_none() {
                watch.native = notify::recommended_watcher(events).ok();
            }
            if let Some(w) = watch.native.as_mut() {
                let _ = w.watch(dir, RecursiveMode::NonRecursive);
            }
        }
    }

    fn unwatch_dir(&self, dir: &Path) {
        let mut watch = self.watch.lock().expect("watch lock");
        watch.resolved.remove(dir);
        if let Some(w) = watch.native.as_mut() {
            let _ = w.unwatch(dir);
        }
        if let Some(w) = watch.poll.as_mut() {
            let _ = w.unwatch(dir);
        }
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
        services.shared.watch_dir(&link);
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
        services.shared.watch_dir(&dir);
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
        let (sink, rx) = collecting();
        let services = Services::start(sink).unwrap();
        for _ in 0..3 {
            services.handle(Request::ListDir(dir.clone()));
            std::thread::sleep(Duration::from_millis(100));
        }
        // Listings come back, but reading the folder reports no change.
        let mut changed = false;
        while let Ok(e) = rx.recv_timeout(Duration::from_millis(600)) {
            changed |= matches!(e, ServiceEvent::Changed(_));
        }
        assert!(!changed, "listing a folder must not report it changed");
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
    fn polling_only_for_wsl_windows_drives() {
        assert!(!needs_polling(Path::new("/home/me/notes")));
        assert_eq!(needs_polling(Path::new("/mnt/c/notes")), is_wsl());
    }
}
