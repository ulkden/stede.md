// SPDX-License-Identifier: GPL-3.0-or-later
//! `stede --probe`: the manual terminal checks left from the phase 0 spike.
//!
//! It prints one sample for each check in `docs/phase-1.md` (truecolor, kitty
//! text sizing, Sixel, kitty graphics), then echoes key presses as STEDE.md
//! sees them, so you can tick what works in each terminal.
//!
//! Every key is echoed raw first, then goes through the keymap in probe mode
//! like any other key: leaving is the `probe.quit` action (`qq` by default).

use std::io::{self, Write};
use std::time::Instant;

use ratatui::crossterm::event::{self, Event};
use ratatui::crossterm::terminal;
use stede_core::action::Registry;
use stede_core::key::{Key, format_keys};
use stede_core::keymap::{Context, Keymap, Mode, Resolver, Scope, Step};
use stede_core::layout::PaneId;

use crate::input::to_key;
use stede_core::app::DEFAULT_KEY_TIMEOUT;

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

/// What a key press did in the probe.
#[derive(Debug, PartialEq, Eq)]
enum Probe {
    /// `probe.quit` ran.
    Quit,
    /// A sequence is pending: wait `key_timeout` for more.
    Wait,
    /// Anything else: keep echoing.
    Continue,
}

/// The probe's keys: probe-mode bindings from the keymap, resolved like the
/// editor's, so the quit key is data and shows up from the keymap.
struct ProbeKeys {
    keymap: Keymap,
    resolver: Resolver,
}

impl ProbeKeys {
    const CONTEXT: Context = Context {
        mode: Mode::Probe,
        pane: PaneId::Raw,
        modal: None,
    };

    fn new() -> Self {
        Self {
            keymap: Keymap::defaults(&Registry::with_builtins()),
            resolver: Resolver::new(),
        }
    }

    /// The keys bound to `probe.quit`, in Vim notation.
    fn quit_keys(&self) -> Option<String> {
        self.keymap
            .keys_for(Mode::Probe, Scope::Global, "probe.quit")
            .map(|keys| format_keys(&keys))
    }

    fn press(&mut self, key: Key) -> Probe {
        let step = self.resolver.feed(&self.keymap, Self::CONTEXT, key);
        Self::outcome(step)
    }

    fn timeout(&mut self) -> Probe {
        match self.resolver.timeout(&self.keymap, Self::CONTEXT) {
            Some(step) => Self::outcome(step),
            None => Probe::Continue,
        }
    }

    fn outcome(step: Step) -> Probe {
        match step {
            Step::Run { action, .. } if action.name == "probe.quit" => Probe::Quit,
            Step::Pending => Probe::Wait,
            _ => Probe::Continue,
        }
    }
}

/// Runs the probe. The `probe.quit` action (`qq` by default) leaves.
pub fn run() -> io::Result<()> {
    let mut keys = ProbeKeys::new();
    // With no way out, raw mode would trap the shell: refuse to start.
    let quit = keys.quit_keys().ok_or_else(|| {
        io::Error::other("no key is bound to probe.quit, so the probe couldn't be left")
    })?;
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
         Press {quit} in quick succession to quit."
    )?;
    out.flush()?;

    let _raw = RawMode::enable()?;
    echo_keys(&mut out, &mut keys)?;
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

/// Echoes each key event raw (what the terminal sent and what STEDE.md
/// makes of it), then resolves it through the probe keymap.
fn echo_keys(out: &mut impl Write, keys: &mut ProbeKeys) -> io::Result<()> {
    let mut deadline: Option<Instant> = None;
    loop {
        let event = match deadline {
            Some(at) => {
                if event::poll(at.saturating_duration_since(Instant::now()))? {
                    Some(event::read()?)
                } else {
                    None
                }
            }
            None => Some(event::read()?),
        };
        let outcome = match event {
            None => Some(keys.timeout()),
            Some(Event::Key(ev)) => {
                let key = to_key(ev);
                let seen = key.map_or_else(|| "(ignored)".to_owned(), |k| k.to_string());
                // Raw mode: move to column 0 ourselves.
                write!(out, "\r{:<12} {:?} {:?}\r\n", seen, ev.code, ev.modifiers)?;
                out.flush()?;
                key.map(|k| keys.press(k))
            }
            Some(_) => None,
        };
        match outcome {
            Some(Probe::Quit) => return Ok(()),
            Some(Probe::Wait) => deadline = Some(Instant::now() + DEFAULT_KEY_TIMEOUT),
            Some(Probe::Continue) => deadline = None,
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_goes_through_the_probe_keymap() {
        let mut keys = ProbeKeys::new();
        assert_eq!(keys.quit_keys().as_deref(), Some("qq"));
        assert_eq!(keys.press(Key::char('q')), Probe::Wait);
        assert_eq!(keys.press(Key::char('q')), Probe::Quit);
        // Anything else in between resets the sequence.
        assert_eq!(keys.press(Key::char('q')), Probe::Wait);
        assert_eq!(keys.press(Key::char('x')), Probe::Continue);
        assert_eq!(keys.press(Key::char('q')), Probe::Wait);
        // A pause too long drops the pending `q`.
        assert_eq!(keys.timeout(), Probe::Continue);
        assert_eq!(keys.press(Key::char('q')), Probe::Wait);
        // Editor keys do nothing here: only probe-mode bindings apply.
        assert_eq!(keys.press(Key::ctrl('l')), Probe::Continue);
    }

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
