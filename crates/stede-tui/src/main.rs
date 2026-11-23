// SPDX-License-Identifier: GPL-3.0-or-later
//! `stede`: the STEDE.md terminal front end.
//!
//! Owns the event loop: reads crossterm events, hands keys to the core, and
//! draws the view model the core returns. `ratatui::try_init` puts the terminal
//! in raw mode on the alternate screen and installs a panic hook that
//! restores it; `ratatui::restore` undoes it on every normal exit path.

mod input;
mod probe;
mod ui;

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Instant;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};
use stede_core::config::{self, Config, ConfigError};
use stede_core::io::ServiceEvent;
use stede_core::layout::LayoutOptions;
use stede_core::{App, AppOptions};
use stede_markdown::Flavor;
use stede_services::{Services, Sink};

const USAGE: &str = "\
Usage: stede [PATH]
       stede --probe

Open a folder or a Markdown file. With a single file, the file tree starts folded.

Options:
  --probe     Check what this terminal supports (colors, text sizing, images, keys)
  -h, --help  Show this help
  -V, --version";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Open(Option<PathBuf>),
    Probe,
    Help,
    Version,
}

fn parse_args(args: &[String]) -> Result<Command, String> {
    let mut path = None;
    let mut options_done = false;
    for arg in args {
        match arg.as_str() {
            "--" if !options_done => options_done = true,
            p if options_done => {
                if path.replace(PathBuf::from(p)).is_some() {
                    return Err("only one path can be opened".into());
                }
            }
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "--probe" => return Ok(Command::Probe),
            flag if flag.starts_with('-') && flag != "-" => {
                return Err(format!("unknown option {flag}"));
            }
            p if path.is_none() => path = Some(PathBuf::from(p)),
            _ => return Err("only one path can be opened".into()),
        }
    }
    Ok(Command::Open(path))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match parse_args(&args) {
        Ok(Command::Help) => {
            println!("{USAGE}");
            Ok(())
        }
        Ok(Command::Version) => {
            println!("{} {}", stede_core::SHORT_NAME, env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Ok(Command::Probe) => {
            let (config, errors) = load_config(None);
            probe::run(&config, &errors)
        }
        Ok(Command::Open(path)) => open(path),
        Err(e) => {
            eprintln!("stede: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("stede: {e}");
            ExitCode::FAILURE
        }
    }
}

fn open(path: Option<PathBuf>) -> io::Result<()> {
    let (root, file) = match &path {
        // metadata, not exists(): a permission error says so instead of
        // looking like a missing path.
        Some(p) => match std::fs::metadata(p) {
            Ok(meta) => {
                // `..` resolved, so the tree's title and paths read cleanly.
                let abs = config::lexical_normal(&std::path::absolute(p)?);
                if meta.is_file() {
                    let parent = abs
                        .parent()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| abs.clone());
                    (parent, Some(abs))
                } else {
                    (abs, None)
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(io::Error::new(
                    e.kind(),
                    format!("{}: no such file or folder", p.display()),
                ));
            }
            Err(e) => return Err(io::Error::new(e.kind(), format!("{}: {e}", p.display()))),
        },
        None => (config::lexical_normal(&std::env::current_dir()?), None),
    };
    let (config, config_errors) = load_config(path.as_deref());
    let mut theme = theme_for(&config);
    let mut app = App::new(AppOptions {
        layout: LayoutOptions {
            tree_folded: file.is_some(),
            ..LayoutOptions::default()
        },
        config,
        config_errors,
        root: Some(root),
        file,
    });

    // One channel for everything the loop waits on: keys from the input
    // thread and results from the services.
    let (tx, rx) = mpsc::sync_channel::<AppEvent>(256);
    let input_tx = tx.clone();
    thread::Builder::new()
        .name("stede-input".into())
        .spawn(move || {
            loop {
                let event = match event::read() {
                    Ok(event) => AppEvent::Input(event),
                    // Pass the error on, so the app exits with it rather than
                    // waiting forever for keys that can't come.
                    Err(e) => {
                        let _ = input_tx.send(AppEvent::InputError(e));
                        break;
                    }
                };
                if input_tx.send(event).is_err() {
                    break;
                }
            }
        })?;
    let sink: Sink = Arc::new(move |event| {
        let _ = tx.send(AppEvent::Service(event));
    });
    let services = Services::start(sink)?;
    let start = path.clone().unwrap_or_else(|| PathBuf::from("."));
    services.watch_config(config_files(Some(&start)));

    // try_init returns an error (no TTY, say) instead of panicking, and installs
    // the panic hook that restores the terminal.
    let mut terminal = ratatui::try_init()?;
    let result = run(&mut terminal, &mut app, &mut theme, &rx, &services);
    ratatui::restore();
    result
}

/// What the loop waits on.
enum AppEvent {
    Input(Event),
    /// Reading the terminal failed; the input thread has stopped.
    InputError(io::Error),
    Service(ServiceEvent),
}

fn theme_for(config: &Config) -> ui::Theme {
    ui::Theme::from_core(
        &config.theme,
        ui::ColorSupport::detect(
            std::env::var("COLORTERM").ok().as_deref(),
            std::env::var("TERM").ok().as_deref(),
        ),
    )
}

/// Flavors in the plan (CLAUDE.md) that aren't built yet.
const PLANNED_FLAVORS: &[&str] = &["obsidian", "mkdocs"];

/// Reads the config layers for an opened path (or the current folder) and
/// merges them. The flavor name is checked here, by the Markdown crate.
/// The config files for an opened path (or the current folder), in layer
/// order, existing or not: what's read at start and watched after.
fn config_files(path: Option<&std::path::Path>) -> Vec<PathBuf> {
    let start = path.unwrap_or_else(|| std::path::Path::new("."));
    config::config_candidates(start, |k| std::env::var(k).ok())
}

/// Reads and merges the config layers (at start; reloads get their layers
/// from the services, read off the input loop).
fn load_config(path: Option<&std::path::Path>) -> (Config, Vec<ConfigError>) {
    let (layers, errors) = config::read_layers(&config_files(path));
    merge_config(&layers, errors)
}

/// Merges config layers and checks what the core can't: the flavor name.
/// No I/O, so it's safe on the input loop.
fn merge_config(
    layers: &[config::Layer],
    read_errors: Vec<ConfigError>,
) -> (Config, Vec<ConfigError>) {
    let mut errors = read_errors;
    let (config, more) = Config::from_layers(layers);
    errors.extend(more);
    if let Some((name, origin)) = &config.flavor
        && let Err(e) = name.parse::<Flavor>()
    {
        // Flavors later in the plan are notes, like other planned settings.
        let mut problem = origin.error(e);
        if PLANNED_FLAVORS.contains(&name.to_ascii_lowercase().as_str()) {
            problem.severity = config::Severity::Note;
            problem.message = format!("the {name} flavor isn't built yet; using gfm");
        }
        errors.push(problem);
    }
    // Show paths under the home folder as `~/...`: shorter, and the same as
    // people write them. Every path a problem can name is rewritten, so one
    // file never shows under two names.
    let mut config = config;
    if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        let tilde = |file: &mut std::path::PathBuf| {
            if let Ok(rest) = file.strip_prefix(&home) {
                *file = std::path::Path::new("~").join(rest);
            }
        };
        errors.iter_mut().for_each(|e| tilde(&mut e.file));
        config
            .bindings
            .iter_mut()
            .for_each(|b| tilde(&mut b.origin.file));
        if let Some((_, origin)) = &mut config.flavor {
            tilde(&mut origin.file);
        }
    }
    (config, errors)
}

fn run(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    theme: &mut ui::Theme,
    events: &mpsc::Receiver<AppEvent>,
    services: &Services,
) -> io::Result<()> {
    // When a key sequence is ambiguous: the moment its timeout runs out. Other
    // events (resize, service results) don't push the deadline back.
    let mut deadline: Option<Instant> = None;
    loop {
        for request in app.take_requests() {
            services.handle(request);
        }
        let size = terminal.size()?;
        app.resize(size.width, size.height);
        terminal.draw(|frame| ui::draw(frame, &app.view(), theme))?;
        if app.should_quit() {
            return Ok(());
        }
        let event = match deadline {
            Some(at) => match events.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(event) => Some(event),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            },
            None => match events.recv() {
                Ok(event) => Some(event),
                Err(_) => return Ok(()),
            },
        };
        let handled = match event {
            None => Some(app.handle_timeout()),
            Some(AppEvent::Input(Event::Key(key))) => {
                input::to_key(key).map(|key| app.handle_key(key))
            }
            // Resize and other input: just draw again.
            Some(AppEvent::Input(_)) => None,
            Some(AppEvent::InputError(e)) => return Err(e),
            Some(AppEvent::Service(ServiceEvent::ConfigChanged { layers, errors })) => {
                // Live reload: the services read the layers again; merging
                // them is pure. Problems show as at start.
                let (config, errors) = merge_config(&layers, errors);
                *theme = theme_for(&config);
                app.apply_config(&config, &errors);
                None
            }
            Some(AppEvent::Service(event)) => {
                app.handle_event(event);
                None
            }
        };
        if let Some(handled) = handled {
            deadline = handled.wait.then(|| Instant::now() + app.key_timeout());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_arguments() {
        assert_eq!(parse_args(&args(&[])), Ok(Command::Open(None)));
        assert_eq!(
            parse_args(&args(&["notes.md"])),
            Ok(Command::Open(Some("notes.md".into())))
        );
        assert_eq!(parse_args(&args(&["--probe"])), Ok(Command::Probe));
        assert_eq!(parse_args(&args(&["-h"])), Ok(Command::Help));
        assert_eq!(parse_args(&args(&["-V"])), Ok(Command::Version));
        assert!(parse_args(&args(&["--nope"])).is_err());
        assert_eq!(
            parse_args(&args(&["--", "-draft.md"])),
            Ok(Command::Open(Some("-draft.md".into())))
        );
        assert_eq!(
            parse_args(&args(&["--", "--"])),
            Ok(Command::Open(Some("--".into())))
        );
        assert!(parse_args(&args(&["--", "a", "b"])).is_err());
        assert!(parse_args(&args(&["a", "b"])).is_err());
    }
}
