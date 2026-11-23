// SPDX-License-Identifier: GPL-3.0-or-later
//! `stede`: the STEDE.md terminal front end.
//!
//! Owns the event loop: reads crossterm events, hands keys to the core, and
//! draws the view model the core returns. `ratatui::init` puts the terminal
//! in raw mode on the alternate screen and installs a panic hook that
//! restores it; `ratatui::restore` undoes it on every normal exit path.

mod input;
mod probe;
mod ui;

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

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
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    let theme = ui::Theme::default();
    let mut waiting = false;
    loop {
        terminal.draw(|frame| ui::draw(frame, &app.view(frame.area().width), &theme))?;
        if app.should_quit() {
            return Ok(());
        }
        // While a key sequence is ambiguous, wait only `key_timeout` for more.
        let event = if waiting {
            if event::poll(app.key_timeout())? {
                Some(event::read()?)
            } else {
                None
            }
        } else {
            Some(event::read()?)
        };
        match event {
            None => waiting = app.handle_timeout().wait,
            Some(Event::Key(key)) => {
                if let Some(key) = input::to_key(key) {
                    waiting = app.handle_key(key).wait;
                }
            }
            // Resize and everything else: just draw again.
            Some(_) => {}
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
