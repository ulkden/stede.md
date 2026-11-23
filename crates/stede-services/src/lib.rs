// SPDX-License-Identifier: GPL-3.0-or-later
//! Background services for STEDE.md: folder listings and file watching
//! (polling on `/mnt/*` paths in WSL, where inotify doesn't fire). Later, the
//! linter hub and external commands.
//!
//! Work runs on a tokio runtime (`spawn_blocking` for file system calls) and
//! reports back through a sink the front end provides, so nothing here
//! blocks the input loop. No UI code here.

mod files;

pub use files::list_dir;

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use notify::{PollWatcher, RecommendedWatcher, RecursiveMode, Watcher};
use stede_core::io::{Request, ServiceEvent};

/// Where results go. Called from service threads; it may block briefly
/// (a bounded channel), never for long.
pub type Sink = Arc<dyn Fn(ServiceEvent) + Send + Sync>;

/// How long changes collect before they're reported as one event, so a
/// burst (a `git checkout`, a build) costs one re-list per folder.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// How often the polling watcher (WSL `/mnt/*` paths) checks for changes.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// The running services.
pub struct Services {
    runtime: tokio::runtime::Runtime,
    sink: Sink,
    watch: Mutex<Watch>,
    config_files: Arc<Mutex<HashSet<PathBuf>>>,
}

/// File watchers, made on first use, and what they watch.
struct Watch {
    events: mpsc::Sender<notify::Result<notify::Event>>,
    native: Option<RecommendedWatcher>,
    poll: Option<PollWatcher>,
    watched: HashSet<PathBuf>,
}

impl Services {
    /// Starts the runtime and the change debouncer.
    pub fn start(sink: Sink) -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("stede-services")
            .build()?;
        let (events, raw) = mpsc::channel();
        let config_files = Arc::new(Mutex::new(HashSet::new()));
        {
            let sink = Arc::clone(&sink);
            let config_files = Arc::clone(&config_files);
            thread::Builder::new()
                .name("stede-watch".into())
                .spawn(move || debounce(&raw, &sink, &config_files))?;
        }
        Ok(Self {
            runtime,
            sink,
            watch: Mutex::new(Watch {
                events,
                native: None,
                poll: None,
                watched: HashSet::new(),
            }),
            config_files,
        })
    }

    /// Takes a request from the core.
    pub fn handle(&self, request: Request) {
        match request {
            Request::ListDir(dir) => {
                // Watch first, so a change during the listing isn't missed.
                self.watch_dir(&dir);
                let sink = Arc::clone(&self.sink);
                self.runtime.spawn_blocking(move || {
                    let result = list_dir(&dir);
                    sink(ServiceEvent::Listed { dir, result });
                });
            }
            // Loading files into the panes comes with the reader panes.
            Request::OpenFile(_) => {}
        }
    }

    /// Watches config files, reporting `ConfigChanged` when one changes.
    /// Their folders are watched, so a file that doesn't exist yet counts
    /// once it's created.
    pub fn watch_config(&self, files: &[PathBuf]) {
        {
            let mut set = self.config_files.lock().expect("config set lock");
            set.extend(files.iter().cloned());
        }
        for dir in files.iter().filter_map(|f| f.parent()) {
            if dir.is_dir() {
                self.watch_dir(dir);
            }
        }
    }

    fn watch_dir(&self, dir: &Path) {
        let mut watch = self.watch.lock().expect("watch lock");
        if !watch.watched.insert(dir.to_path_buf()) {
            return;
        }
        let events = watch.events.clone();
        let result = if needs_polling(dir) {
            if watch.poll.is_none() {
                let config = notify::Config::default().with_poll_interval(POLL_INTERVAL);
                watch.poll = PollWatcher::new(events, config).ok();
            }
            watch
                .poll
                .as_mut()
                .map(|w| w.watch(dir, RecursiveMode::NonRecursive))
        } else {
            if watch.native.is_none() {
                watch.native = notify::recommended_watcher(events).ok();
            }
            watch
                .native
                .as_mut()
                .map(|w| w.watch(dir, RecursiveMode::NonRecursive))
        };
        // A folder that can't be watched still lists; it just won't refresh
        // on its own. Forget it so a later listing tries again.
        if !matches!(result, Some(Ok(()))) {
            watch.watched.remove(dir);
        }
    }
}

/// Collects raw watcher events and reports them in batches: the folders that
/// changed, and whether a config file did.
fn debounce(
    raw: &mpsc::Receiver<notify::Result<notify::Event>>,
    sink: &Sink,
    config_files: &Mutex<HashSet<PathBuf>>,
) {
    // Ends when every sender (the watchers and `Services`) is gone.
    while let Ok(first) = raw.recv() {
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut config_changed = false;
        let mut take = |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            let configs = config_files.lock().expect("config set lock");
            for path in event.paths {
                if configs.contains(&path) {
                    config_changed = true;
                }
                if !dirs.contains(&path) {
                    dirs.push(path);
                }
            }
        };
        take(first);
        let until = Instant::now() + DEBOUNCE;
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match raw.recv_timeout(left) {
                Ok(event) => take(event),
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        if !dirs.is_empty() {
            sink(ServiceEvent::Changed(dirs));
        }
        if config_changed {
            sink(ServiceEvent::ConfigChanged);
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
    fn config_changes_are_reported() {
        let dir = temp("config");
        let file = dir.join("config.toml");
        let (sink, rx) = collecting();
        let services = Services::start(sink).unwrap();
        services.watch_config(std::slice::from_ref(&file));
        std::fs::write(&file, "[ui]\n").unwrap();
        assert!(wait_for(&rx, |e| *e == ServiceEvent::ConfigChanged));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn polling_only_for_wsl_windows_drives() {
        assert!(!needs_polling(Path::new("/home/me/notes")));
        assert_eq!(needs_polling(Path::new("/mnt/c/notes")), is_wsl());
    }
}
