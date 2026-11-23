// SPDX-License-Identifier: GPL-3.0-or-later
//! Keys as the core sees them.
//!
//! Front ends translate their own key events into [`Key`] values; the core
//! never sees a crossterm (or iced) type. Keymaps write keys in Vim notation
//! (`gg`, `<C-w>l`, `<leader>p`, `<Esc>`), parsed by [`parse_keys`].

use std::fmt;

/// A key, without modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum KeyCode {
    /// A printable character, already in its shifted form (`G`, not Shift+`g`).
    Char(char),
    Enter,
    Esc,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Insert,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// A function key, `F(1)` to `F(24)`.
    F(u8),
}

/// Modifier keys held with a [`KeyCode`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        ctrl: false,
        alt: false,
        shift: false,
    };
    pub const CTRL: Self = Self {
        ctrl: true,
        alt: false,
        shift: false,
    };
}

/// One key press: a code plus modifiers, normalized so equal presses compare equal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key {
    code: KeyCode,
    mods: Modifiers,
}

impl Key {
    /// Builds a normalized key.
    ///
    /// - Shift on a character is dropped: the character already says it (`G`).
    /// - Ctrl with a letter uses the lowercase letter, so `<C-W>` equals `<C-w>`.
    pub fn new(code: KeyCode, mods: Modifiers) -> Self {
        let mut mods = mods;
        let code = match code {
            KeyCode::Char(c) => {
                mods.shift = false;
                if mods.ctrl && c.is_ascii_alphabetic() {
                    KeyCode::Char(c.to_ascii_lowercase())
                } else {
                    KeyCode::Char(c)
                }
            }
            other => other,
        };
        Self { code, mods }
    }

    /// A plain character key.
    pub fn char(c: char) -> Self {
        Self::new(KeyCode::Char(c), Modifiers::NONE)
    }

    /// Ctrl plus a character.
    pub fn ctrl(c: char) -> Self {
        Self::new(KeyCode::Char(c), Modifiers::CTRL)
    }

    /// A key with no modifiers.
    pub fn plain(code: KeyCode) -> Self {
        Self::new(code, Modifiers::NONE)
    }

    pub fn code(&self) -> KeyCode {
        self.code
    }

    pub fn mods(&self) -> Modifiers {
        self.mods
    }

    /// The digit this key types, if it is a bare `0`–`9`.
    pub fn digit(&self) -> Option<u32> {
        match self.code {
            KeyCode::Char(c) if self.mods == Modifiers::NONE => c.to_digit(10),
            _ => None,
        }
    }
}

impl fmt::Display for Key {
    /// Vim notation: `a`, `G`, `<Space>`, `<C-w>`, `<S-Tab>`, `<F1>`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self.code {
            KeyCode::Char(' ') => "Space".to_owned(),
            KeyCode::Char('<') => "lt".to_owned(),
            KeyCode::Char('\\') => "Bslash".to_owned(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Enter => "CR".to_owned(),
            KeyCode::Esc => "Esc".to_owned(),
            KeyCode::Tab => "Tab".to_owned(),
            KeyCode::BackTab => "S-Tab".to_owned(),
            KeyCode::Backspace => "BS".to_owned(),
            KeyCode::Delete => "Del".to_owned(),
            KeyCode::Insert => "Insert".to_owned(),
            KeyCode::Up => "Up".to_owned(),
            KeyCode::Down => "Down".to_owned(),
            KeyCode::Left => "Left".to_owned(),
            KeyCode::Right => "Right".to_owned(),
            KeyCode::Home => "Home".to_owned(),
            KeyCode::End => "End".to_owned(),
            KeyCode::PageUp => "PageUp".to_owned(),
            KeyCode::PageDown => "PageDown".to_owned(),
            KeyCode::F(n) => format!("F{n}"),
        };
        let plain_char = matches!(self.code, KeyCode::Char(c) if c != ' ' && c != '<' && c != '\\');
        let mut prefix = String::new();
        if self.mods.ctrl {
            prefix.push_str("C-");
        }
        if self.mods.alt {
            prefix.push_str("A-");
        }
        if self.mods.shift {
            prefix.push_str("S-");
        }
        if plain_char && prefix.is_empty() {
            write!(f, "{name}")
        } else {
            write!(f, "<{prefix}{name}>")
        }
    }
}

/// Formats a key sequence in Vim notation, e.g. `gg` or `<C-w>l`.
pub fn format_keys(keys: &[Key]) -> String {
    keys.iter().map(Key::to_string).collect()
}

/// Why a key string could not be parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyParseError {
    Empty,
    /// `<...>` held a name we don't know, such as `<Foo>`.
    UnknownName(String),
    /// `<leader>` was used but no leader key is set.
    NoLeader,
}

impl fmt::Display for KeyParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "empty key sequence"),
            Self::UnknownName(name) => write!(f, "unknown key name <{name}>"),
            Self::NoLeader => write!(f, "<leader> is used but no leader key is set"),
        }
    }
}

impl std::error::Error for KeyParseError {}

/// Parses Vim key notation into keys.
///
/// Plain characters stand for themselves; `<...>` names a special key or adds
/// modifiers (`<C-w>`, `<A-x>`, `<S-Tab>`, `<F5>`, `<Space>`, `<lt>`).
/// `<leader>` expands to `leader`. A `<` that doesn't start a valid name is a
/// literal `<`.
pub fn parse_keys(s: &str, leader: Option<Key>) -> Result<Vec<Key>, KeyParseError> {
    let chars: Vec<char> = s.chars().collect();
    let mut keys = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let close = if chars[i] == '<' {
            chars[i + 1..]
                .iter()
                .position(|&c| c == '>')
                .filter(|&len| len > 0)
        } else {
            None
        };
        if let Some(len) = close {
            let inner: String = chars[i + 1..i + 1 + len].iter().collect();
            if inner.eq_ignore_ascii_case("leader") {
                keys.push(leader.ok_or(KeyParseError::NoLeader)?);
                i += len + 2;
                continue;
            }
            match parse_named(&inner) {
                Some(key) => {
                    keys.push(key);
                    i += len + 2;
                    continue;
                }
                // `<` followed by something that only looks like a name is an
                // error when it has the shape of one (letters, digits, dashes).
                None if inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') => {
                    return Err(KeyParseError::UnknownName(inner));
                }
                None => {}
            }
        }
        keys.push(Key::char(chars[i]));
        i += 1;
    }
    if keys.is_empty() {
        return Err(KeyParseError::Empty);
    }
    Ok(keys)
}

/// Parses the inside of `<...>`: optional `C-`, `A-`/`M-`, `S-` prefixes and a name.
fn parse_named(inner: &str) -> Option<Key> {
    let mut mods = Modifiers::NONE;
    let mut rest = inner;
    loop {
        let bytes = rest.as_bytes();
        if bytes.len() > 2 && bytes[1] == b'-' {
            match bytes[0].to_ascii_lowercase() {
                b'c' => mods.ctrl = true,
                b'a' | b'm' => mods.alt = true,
                b's' => mods.shift = true,
                _ => break,
            }
            rest = &rest[2..];
        } else {
            break;
        }
    }
    let code = match rest.to_ascii_lowercase().as_str() {
        "esc" => KeyCode::Esc,
        "cr" | "enter" | "return" => KeyCode::Enter,
        "tab" if mods.shift => {
            mods.shift = false;
            KeyCode::BackTab
        }
        "tab" => KeyCode::Tab,
        "bs" | "backspace" => KeyCode::Backspace,
        "del" | "delete" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "space" => KeyCode::Char(' '),
        "lt" => KeyCode::Char('<'),
        "bslash" => KeyCode::Char('\\'),
        name if name.starts_with('f') && name.len() > 1 => {
            let n: u8 = name[1..].parse().ok()?;
            if (1..=24).contains(&n) {
                KeyCode::F(n)
            } else {
                return None;
            }
        }
        _ => {
            // A single character with modifiers, such as `<C-w>` or `<A-j>`.
            let mut it = rest.chars();
            let c = it.next()?;
            if it.next().is_some() || mods == Modifiers::NONE {
                return None;
            }
            KeyCode::Char(c)
        }
    };
    Some(Key::new(code, mods))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(s: &str) -> Vec<Key> {
        parse_keys(s, Some(Key::char(' '))).unwrap()
    }

    #[test]
    fn plain_sequences() {
        assert_eq!(keys("gg"), vec![Key::char('g'), Key::char('g')]);
        assert_eq!(keys("G"), vec![Key::char('G')]);
    }

    #[test]
    fn named_and_modified_keys() {
        assert_eq!(keys("<Esc>"), vec![Key::plain(KeyCode::Esc)]);
        assert_eq!(keys("<C-w>l"), vec![Key::ctrl('w'), Key::char('l')]);
        assert_eq!(keys("<C-W>"), vec![Key::ctrl('w')]);
        assert_eq!(keys("<S-Tab>"), vec![Key::plain(KeyCode::BackTab)]);
        assert_eq!(keys("<F1>"), vec![Key::plain(KeyCode::F(1))]);
        assert_eq!(keys("<cr>"), vec![Key::plain(KeyCode::Enter)]);
        assert_eq!(
            keys("<A-j>"),
            vec![Key::new(
                KeyCode::Char('j'),
                Modifiers {
                    alt: true,
                    ..Modifiers::NONE
                }
            )]
        );
    }

    #[test]
    fn leader_expands() {
        assert_eq!(keys("<leader>p"), vec![Key::char(' '), Key::char('p')]);
        assert_eq!(parse_keys("<leader>p", None), Err(KeyParseError::NoLeader));
    }

    #[test]
    fn literal_angle_brackets() {
        assert_eq!(keys("<"), vec![Key::char('<')]);
        assert_eq!(keys("<lt>"), vec![Key::char('<')]);
        assert_eq!(keys("<>"), vec![Key::char('<'), Key::char('>')]);
        assert_eq!(
            keys("a<b"),
            vec![Key::char('a'), Key::char('<'), Key::char('b')]
        );
    }

    #[test]
    fn errors() {
        assert_eq!(parse_keys("", None), Err(KeyParseError::Empty));
        assert_eq!(
            parse_keys("<Foo>", None),
            Err(KeyParseError::UnknownName("Foo".into()))
        );
        assert_eq!(
            parse_keys("<F99>", None),
            Err(KeyParseError::UnknownName("F99".into()))
        );
    }

    #[test]
    fn shift_on_chars_is_dropped() {
        let shifted = Key::new(
            KeyCode::Char('G'),
            Modifiers {
                shift: true,
                ..Modifiers::NONE
            },
        );
        assert_eq!(shifted, Key::char('G'));
    }

    #[test]
    fn display_round_trips() {
        for s in [
            "gg", "<C-w>l", "<Space>p", "<Esc>", "<S-Tab>", "<F12>", "<lt>", "<A-x>", "?",
        ] {
            let parsed = keys(s);
            assert_eq!(
                parse_keys(&format_keys(&parsed), None).unwrap(),
                parsed,
                "{s}"
            );
        }
        assert_eq!(format_keys(&keys("<leader>?")), "<Space>?");
    }

    #[test]
    fn digits() {
        assert_eq!(Key::char('3').digit(), Some(3));
        assert_eq!(Key::ctrl('3').digit(), None);
        assert_eq!(Key::char('x').digit(), None);
    }
}
