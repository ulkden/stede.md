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
use std::time::Instant;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};
use stede_core::layout::LayoutOptions;
use stede_core::{App, AppOptions};

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
    for arg in args {
        match arg.as_str() {
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
        Ok(Command::Probe) => probe::run(),
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
    let tree_folded = match &path {
        Some(p) if !p.exists() => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{}: no such file or folder", p.display()),
            ));
        }
        Some(p) => p.is_file(),
        None => false,
    };
    let mut app = App::new(AppOptions {
        layout: LayoutOptions {
            tree_folded,
            ..LayoutOptions::default()
        },
    });
    if let Some(p) = &path {
        // Loading files arrives in phase 1 step 5 (docs/phase-1.md).
        app.notify(format!("{}: opening files isn't built yet", p.display()));
    }
    // try_init returns an error (no TTY, say) instead of panicking, and installs
    // the panic hook that restores the terminal.
    let mut terminal = ratatui::try_init()?;
    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    let theme = ui::Theme::default();
    // When a key sequence is ambiguous: the moment its timeout runs out. Other
    // events (resize, focus, unnamed keys) don't push the deadline back.
    let mut deadline: Option<Instant> = None;
    loop {
        let size = terminal.size()?;
        app.resize(size.width, size.height);
        terminal.draw(|frame| ui::draw(frame, &app.view(), &theme))?;
        if app.should_quit() {
            return Ok(());
        }
        let event = match deadline {
            Some(at) => {
                let left = at.saturating_duration_since(Instant::now());
                if event::poll(left)? {
                    Some(event::read()?)
                } else {
                    None
                }
            }
            None => Some(event::read()?),
        };
        let handled = match event {
            None => Some(app.handle_timeout()),
            Some(Event::Key(key)) => input::to_key(key).map(|key| app.handle_key(key)),
            // Resize and everything else: just draw again.
            Some(_) => None,
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
        assert!(parse_args(&args(&["a", "b"])).is_err());
    }
}
