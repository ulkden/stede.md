// SPDX-License-Identifier: GPL-3.0-or-later
//! `stede --probe`: the manual terminal checks left from the phase 0 spike.
//!
//! It prints one sample for each check in `docs/phase-1.md` (truecolor, kitty
//! text sizing, Sixel, kitty graphics), then echoes key presses as STEDE.md
//! sees them, so you can tick what works in each terminal.

use std::io::{self, Write};

use ratatui::crossterm::event::{self, Event, KeyCode};
use ratatui::crossterm::terminal;

use crate::input::to_key;

/// The samples, each a label and the bytes that draw it.
fn samples() -> Vec<(&'static str, String)> {
    let gradient: String = (0..48)
        .map(|i| {
            let r = 255 - i * 5;
            let b = i * 5;
            format!("\x1b[48;2;{r};64;{b}m ")
        })
        .collect::<String>()
        + "\x1b[0m";
    vec![
        ("Truecolor: a smooth red-to-blue band, no stripes", gradient),
        (
            "kitty text sizing (OSC 66): the word below is twice the normal size",
            "\x1b]66;s=2;Heading\x1b\\\n".to_owned(),
        ),
        (
            "Sixel: a small red square",
            "\x1bPq#0;2;100;0;0#0!24~-!24~-!24~\x1b\\".to_owned(),
        ),
        ("kitty graphics: a small green square", kitty_square()),
    ]
}

/// A 16x16 green square in the kitty graphics protocol (raw RGB, base64).
fn kitty_square() -> String {
    let pixels: Vec<u8> = (0..16 * 16).flat_map(|_| [0u8, 200, 80]).collect();
    format!("\x1b_Ga=T,f=24,s=16,v=16;{}\x1b\\", base64(&pixels))
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Runs the probe. Press `q` twice in a row to leave.
pub fn run() -> io::Result<()> {
    let mut out = io::stdout();
    writeln!(
        out,
        "STEDE.md terminal probe. Tick results in docs/phase-1.md.\n"
    )?;
    for (label, bytes) in samples() {
        writeln!(out, "{label}")?;
        write!(out, "{bytes}")?;
        writeln!(out, "\n")?;
    }
    writeln!(
        out,
        "Keys: press <C-w>, <C-h>, <C-l>, <F1>, <Esc>, <S-Tab>, <A-x> and check each shows as named.\n\
         Press q twice to quit."
    )?;
    out.flush()?;

    let _raw = RawMode::enable()?;
    echo_keys(&mut out)?;
    writeln!(out)?;
    Ok(())
}

/// Raw mode for the probe, turned off when dropped, including on a panic,
/// so the shell isn't left without echo.
struct RawMode;

impl RawMode {
    fn enable() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

/// Echoes raw key events. This is the one place keys don't go through the
/// keymap and action registry, on purpose: the probe shows what the terminal
/// sends before any binding applies, so its quit key (`q q`) is fixed.
fn echo_keys(out: &mut impl Write) -> io::Result<()> {
    let mut last_q = false;
    loop {
        if let Event::Key(ev) = event::read()? {
            let seen = to_key(ev).map_or_else(|| "(ignored)".to_owned(), |k| k.to_string());
            // Raw mode: move to column 0 ourselves.
            write!(out, "\r{:<12} {:?} {:?}\r\n", seen, ev.code, ev.modifiers)?;
            out.flush()?;
            let is_q = ev.code == KeyCode::Char('q') && ev.modifiers.is_empty();
            if is_q && last_q {
                return Ok(());
            }
            last_q = is_q;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Many"), "TWFueQ==");
    }

    #[test]
    fn samples_are_terminated_escapes() {
        for (label, bytes) in samples() {
            assert!(!label.is_empty());
            assert!(
                bytes.ends_with("\x1b\\") || bytes.ends_with("\x1b[0m") || bytes.ends_with('\n')
            );
        }
    }
}
