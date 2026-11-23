// SPDX-License-Identifier: GPL-3.0-or-later
//! Turns crossterm key events into the core's [`Key`] type.
//!
//! This is the only place crossterm's key types meet the core.

use ratatui::crossterm::event::{KeyCode as CtKey, KeyEvent, KeyEventKind, KeyModifiers};
use stede_core::key::{Key, KeyCode, Modifiers};

/// The core key for a crossterm key event, or `None` for releases and keys
/// the core has no name for (media keys, lone modifiers and the like).
pub fn to_key(event: KeyEvent) -> Option<Key> {
    if event.kind == KeyEventKind::Release {
        return None;
    }
    let code = match event.code {
        CtKey::Char(c) => KeyCode::Char(c),
        CtKey::Enter => KeyCode::Enter,
        CtKey::Esc => KeyCode::Esc,
        CtKey::Tab => KeyCode::Tab,
        CtKey::BackTab => KeyCode::BackTab,
        CtKey::Backspace => KeyCode::Backspace,
        CtKey::Delete => KeyCode::Delete,
        CtKey::Insert => KeyCode::Insert,
        CtKey::Up => KeyCode::Up,
        CtKey::Down => KeyCode::Down,
        CtKey::Left => KeyCode::Left,
        CtKey::Right => KeyCode::Right,
        CtKey::Home => KeyCode::Home,
        CtKey::End => KeyCode::End,
        CtKey::PageUp => KeyCode::PageUp,
        CtKey::PageDown => KeyCode::PageDown,
        CtKey::F(n) if (1..=24).contains(&n) => KeyCode::F(n),
        _ => return None,
    };
    let m = event.modifiers;
    Some(Key::new(
        code,
        Modifiers {
            ctrl: m.contains(KeyModifiers::CONTROL),
            alt: m.contains(KeyModifiers::ALT),
            shift: m.contains(KeyModifiers::SHIFT),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEventState;

    fn ev(code: CtKey, modifiers: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn maps_keys_and_modifiers() {
        let press = KeyEventKind::Press;
        assert_eq!(
            to_key(ev(CtKey::Char('w'), KeyModifiers::CONTROL, press)),
            Some(Key::ctrl('w'))
        );
        // Shift+g arrives as `G` with SHIFT; the core folds it to plain `G`.
        assert_eq!(
            to_key(ev(CtKey::Char('G'), KeyModifiers::SHIFT, press)),
            Some(Key::char('G'))
        );
        assert_eq!(
            to_key(ev(CtKey::BackTab, KeyModifiers::SHIFT, press)),
            Some(Key::plain(KeyCode::BackTab))
        );
        assert_eq!(
            to_key(ev(CtKey::F(1), KeyModifiers::NONE, KeyEventKind::Repeat)),
            Some(Key::plain(KeyCode::F(1)))
        );
    }

    #[test]
    fn ignores_releases_and_unnamed_keys() {
        assert_eq!(
            to_key(ev(
                CtKey::Char('a'),
                KeyModifiers::NONE,
                KeyEventKind::Release
            )),
            None
        );
        assert_eq!(
            to_key(ev(CtKey::CapsLock, KeyModifiers::NONE, KeyEventKind::Press)),
            None
        );
    }
}
