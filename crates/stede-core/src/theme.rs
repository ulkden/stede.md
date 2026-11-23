// SPDX-License-Identifier: GPL-3.0-or-later
//! Themes: named color slots, so every color on screen comes from data.
//!
//! A front end asks for a slot (`ui.accent`) and maps the [`Color`] to its own
//! type. Terminal-specific choices, such as downgrading RGB on terminals
//! without truecolor, belong to the front end.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

/// A color as config writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    /// The terminal's own foreground or background.
    Default,
    /// One of the 16 ANSI colors, 0 to 15.
    Ansi(u8),
    Rgb(u8, u8, u8),
}

const ANSI_NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright-black",
    "bright-red",
    "bright-green",
    "bright-yellow",
    "bright-blue",
    "bright-magenta",
    "bright-cyan",
    "bright-white",
];

impl FromStr for Color {
    type Err = String;

    /// `#rrggbb`, `#rgb`, `default`, or an ANSI name (`red`,
    /// `bright-blue`; `gray` is `white` and `dark-gray` is `bright-black`).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if let Some(hex) = s.strip_prefix('#') {
            let digits: Option<Vec<u8>> = hex
                .chars()
                .map(|c| c.to_digit(16).map(|d| d as u8))
                .collect();
            return match digits.as_deref() {
                Some([r1, r2, g1, g2, b1, b2]) => {
                    Ok(Color::Rgb(r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2))
                }
                Some([r, g, b]) => Ok(Color::Rgb(r * 17, g * 17, b * 17)),
                _ => Err(format!("{s:?} isn't a color: use #rrggbb or #rgb")),
            };
        }
        let name = s.to_ascii_lowercase().replace('_', "-");
        match name.as_str() {
            "default" | "reset" => Ok(Color::Default),
            "gray" | "grey" => Ok(Color::Ansi(7)),
            "dark-gray" | "dark-grey" => Ok(Color::Ansi(8)),
            _ => ANSI_NAMES
                .iter()
                .position(|n| *n == name)
                .map(|i| Color::Ansi(i as u8))
                .ok_or_else(|| {
                    format!("{s:?} isn't a color: use #rrggbb, default, or an ANSI name such as red or bright-blue")
                }),
        }
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Color::Default => f.write_str("default"),
            Color::Ansi(i) => f.write_str(ANSI_NAMES[usize::from(*i) & 15]),
            Color::Rgb(r, g, b) => write!(f, "#{r:02x}{g:02x}{b:02x}"),
        }
    }
}

/// Every slot a theme sets, with what it colors.
///
/// A slot is listed here only once something draws it.
pub const SLOTS: &[(&str, &str)] = &[
    (
        "ui.accent",
        "Focused pane border and title, modal borders, keys in the cheat sheet",
    ),
    ("ui.border", "Borders of unfocused panes"),
    (
        "ui.dim",
        "Secondary text: actions in the cheat sheet, modal footers",
    ),
    ("ui.strip.fg", "Text on a folded pane's strip"),
    ("ui.strip.bg", "Background of a folded pane's strip"),
    ("ui.status.fg", "Text of the mode badge in the status line"),
    ("ui.status.bg", "Background of the mode badge"),
];

/// A complete theme: a color for every slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    name: String,
    colors: BTreeMap<&'static str, Color>,
}

/// The built-in themes, by name.
pub const BUILTIN_THEMES: &[&str] = &["dark", "light"];

impl Default for Theme {
    fn default() -> Self {
        Self::builtin("dark").expect("the dark theme is built in")
    }
}

impl Theme {
    /// A built-in theme. `dark` is the default.
    pub fn builtin(name: &str) -> Option<Self> {
        let colors: &[(&str, &str)] = match name {
            "dark" => &[
                ("ui.accent", "#7aa2f7"),
                ("ui.border", "#3b4261"),
                ("ui.dim", "#737aa2"),
                ("ui.strip.fg", "#a9b1d6"),
                ("ui.strip.bg", "#292e42"),
                ("ui.status.fg", "#1a1b26"),
                ("ui.status.bg", "#7aa2f7"),
            ],
            "light" => &[
                ("ui.accent", "#2e5ab8"),
                ("ui.border", "#a8aecb"),
                ("ui.dim", "#6172b0"),
                ("ui.strip.fg", "#3760bf"),
                ("ui.strip.bg", "#d0d5e3"),
                ("ui.status.fg", "#e1e2e7"),
                ("ui.status.bg", "#2e5ab8"),
            ],
            _ => return None,
        };
        let mut theme = Theme {
            name: name.to_owned(),
            colors: BTreeMap::new(),
        };
        for (slot, color) in colors {
            theme
                .set(slot, color.parse().expect("built-in colors parse"))
                .expect("built-in slots exist");
        }
        debug_assert_eq!(theme.colors.len(), SLOTS.len(), "{name} sets every slot");
        Some(theme)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The color for a slot. Unknown slots get the terminal default.
    pub fn get(&self, slot: &str) -> Color {
        self.colors.get(slot).copied().unwrap_or(Color::Default)
    }

    /// Sets one slot. Fails for a slot no theme has.
    pub fn set(&mut self, slot: &str, color: Color) -> Result<(), String> {
        let (known, _) = SLOTS
            .iter()
            .find(|(s, _)| *s == slot)
            .ok_or_else(|| format!("unknown theme slot {slot:?}"))?;
        self.colors.insert(known, color);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colors() {
        assert_eq!("#7aa2f7".parse(), Ok(Color::Rgb(0x7a, 0xa2, 0xf7)));
        assert_eq!("#fff".parse(), Ok(Color::Rgb(255, 255, 255)));
        assert_eq!("bright-blue".parse(), Ok(Color::Ansi(12)));
        assert_eq!("Bright_Blue".parse(), Ok(Color::Ansi(12)));
        assert_eq!("dark-gray".parse(), Ok(Color::Ansi(8)));
        assert_eq!("default".parse(), Ok(Color::Default));
        assert!("#12345".parse::<Color>().is_err());
        assert!("#gggggg".parse::<Color>().is_err());
        assert!("teal".parse::<Color>().is_err());
    }

    #[test]
    fn colors_display_round_trip() {
        for s in ["#7aa2f7", "red", "bright-white", "default"] {
            assert_eq!(s.parse::<Color>().unwrap().to_string(), s);
        }
    }

    #[test]
    fn builtin_themes_set_every_slot() {
        for name in BUILTIN_THEMES {
            let theme = Theme::builtin(name).unwrap();
            for (slot, _) in SLOTS {
                assert!(theme.colors.contains_key(slot), "{name} misses {slot}");
            }
        }
        assert!(Theme::builtin("neon").is_none());
    }

    #[test]
    fn set_checks_slots() {
        let mut t = Theme::default();
        assert!(t.set("ui.accent", Color::Ansi(1)).is_ok());
        assert_eq!(t.get("ui.accent"), Color::Ansi(1));
        assert!(t.set("ui.nope", Color::Ansi(1)).is_err());
    }
}
