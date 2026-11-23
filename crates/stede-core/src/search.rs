// SPDX-License-Identifier: GPL-3.0-or-later
//! Search: a pattern, its matches in the document, and which one is current.
//!
//! Patterns are regular expressions with smart case: no capital letter means
//! case doesn't matter. A pattern that isn't a valid regex is searched as
//! plain text, so a stray `(` still finds itself.

/// The longest pattern typed: each key recompiles it and rescans the file.
pub const MAX_PATTERN: usize = 1024;

/// Matches kept at most, so a one-letter pattern in a huge log can't stall
/// typing; the count then shows as "at least".
pub const MAX_MATCHES: usize = 100_000;

/// Bytes the live preview (each key typed) scans at most, from the view
/// out; Enter scans the rest.
pub const PREVIEW_BYTES: usize = 4 << 20;

/// How a pattern was compiled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compiled {
    Regex,
    /// It wasn't a valid regex, so it's searched as plain text.
    Literal,
    /// Valid, but too big to compile: it matches nothing.
    TooBig,
}

use regex::{Regex, RegexBuilder};

/// Which way `n` goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
}

/// One match: its line (0-based) and byte range in that line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

/// A compiled search over the document's lines.
#[derive(Clone, Debug)]
pub struct Search {
    pub pattern: String,
    pub direction: Direction,
    regex: Regex,
    pub compiled: Compiled,
    matches: Vec<Match>,
    /// Whether matches stopped at [`MAX_MATCHES`].
    pub capped: bool,
    /// Whether only part of the document was scanned (a preview).
    pub partial: bool,
    current: Option<usize>,
}

/// Whether a pattern has a capital letter the user typed to match: letters
/// in escapes (`\W`, `\p{Lu}`) don't count, as in Vim's smartcase.
fn has_capital(pattern: &str) -> bool {
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some('p' | 'P') = chars.next() {
                let mut rest = chars.clone();
                if rest.next() == Some('{') {
                    chars = rest;
                    chars.by_ref().find(|&c| c == '}');
                } else {
                    chars.next();
                }
            }
            continue;
        }
        if c.is_uppercase() {
            return true;
        }
    }
    false
}

/// A regex that matches nothing.
fn nothing() -> Regex {
    Regex::new(r"[^\s\S]").expect("an empty class is a valid regex")
}

/// Builds the regex for a pattern: smart case, and plain text if the
/// pattern doesn't parse.
pub fn compile(pattern: &str) -> (Regex, Compiled) {
    let ignore_case = !has_capital(pattern);
    let build = |p: &str| {
        RegexBuilder::new(p)
            .case_insensitive(ignore_case)
            // A pattern typed by hand shouldn't build a huge automaton.
            .size_limit(1 << 20)
            .build()
    };
    match build(pattern) {
        Ok(regex) => (regex, Compiled::Regex),
        Err(regex::Error::CompiledTooBig(_)) => (nothing(), Compiled::TooBig),
        Err(_) => match build(&regex::escape(pattern)) {
            Ok(regex) => (regex, Compiled::Literal),
            Err(_) => (nothing(), Compiled::TooBig),
        },
    }
}

impl Search {
    /// A search over `lines`. Empty patterns match nothing.
    pub fn new<'a>(
        pattern: &str,
        direction: Direction,
        lines: impl Iterator<Item = &'a str>,
    ) -> Self {
        let (regex, compiled) = compile(pattern);
        let mut search = Self {
            pattern: pattern.to_owned(),
            direction,
            regex,
            compiled,
            matches: Vec::new(),
            capped: false,
            partial: false,
            current: None,
        };
        search.rescan(lines);
        search
    }

    /// A search for the live preview: scans `(index, line)` pairs in the
    /// order given (the view's line first) until [`PREVIEW_BYTES`].
    pub fn preview<'a>(
        pattern: &str,
        direction: Direction,
        lines: impl Iterator<Item = (usize, &'a str)>,
    ) -> Self {
        let mut search = Self::new(pattern, direction, std::iter::empty());
        search.scan(lines, PREVIEW_BYTES);
        search
    }

    /// Finds the matches again (the document changed, or a preview is
    /// confirmed).
    pub fn rescan<'a>(&mut self, lines: impl Iterator<Item = &'a str>) {
        self.scan(lines.enumerate(), usize::MAX);
    }

    fn scan<'a>(&mut self, lines: impl Iterator<Item = (usize, &'a str)>, budget: usize) {
        self.matches.clear();
        self.current = None;
        self.capped = false;
        self.partial = false;
        if self.pattern.is_empty() || self.compiled == Compiled::TooBig {
            return;
        }
        let mut scanned = 0usize;
        let mut sorted = true;
        let mut last = 0;
        for (i, line) in lines {
            if scanned >= budget {
                self.partial = true;
                break;
            }
            scanned = scanned.saturating_add(line.len() + 1);
            sorted &= i >= last;
            last = i;
            for m in self.regex.find_iter(line) {
                if self.matches.len() == MAX_MATCHES {
                    self.capped = true;
                    break;
                }
                // An empty match (`a*`) would light nothing; skip it.
                if m.start() < m.end() {
                    self.matches.push(Match {
                        line: i,
                        start: m.start(),
                        end: m.end(),
                    });
                }
            }
            if self.capped {
                break;
            }
        }
        // Lines given out of order (a preview from the view): steps need them
        // in document order.
        if !sorted {
            self.matches.sort_unstable_by_key(|m| (m.line, m.start));
        }
    }

    /// Whether the count is only a lower bound (capped, or a preview).
    pub fn at_least(&self) -> bool {
        self.capped || self.partial
    }

    pub fn regex(&self) -> &Regex {
        &self.regex
    }

    pub fn matches(&self) -> &[Match] {
        &self.matches
    }

    pub fn current(&self) -> Option<Match> {
        self.current.map(|i| self.matches[i])
    }

    /// Forgets the current match, so the next step starts from a line
    /// (the view was scrolled away from it).
    pub fn clear_current(&mut self) {
        self.current = None;
    }

    /// The current match's place and the count, such as `(3, 12)` (1-based).
    pub fn position(&self) -> Option<(usize, usize)> {
        Some((self.current? + 1, self.matches.len()))
    }

    /// Moves to the next match from line `from` (0-based) going `direction`,
    /// wrapping at the ends. A match on `from` itself counts only if
    /// `inclusive` (a new search: find from here). Returns the match and
    /// whether it wrapped.
    pub fn step(
        &mut self,
        from: usize,
        direction: Direction,
        inclusive: bool,
    ) -> Option<(Match, bool)> {
        if self.matches.is_empty() {
            return None;
        }
        // From the current match when there is one, so repeated `n` moves on
        // even when several matches share a line.
        let (index, wrapped) = match (direction, self.current) {
            (Direction::Forward, Some(c)) if !inclusive => {
                let next = c + 1;
                if next < self.matches.len() {
                    (next, false)
                } else {
                    (0, true)
                }
            }
            (Direction::Backward, Some(c)) if !inclusive => {
                if c > 0 {
                    (c - 1, false)
                } else {
                    (self.matches.len() - 1, true)
                }
            }
            (Direction::Forward, _) => {
                let at = self.matches.partition_point(|m| {
                    if inclusive {
                        m.line < from
                    } else {
                        m.line <= from
                    }
                });
                if at < self.matches.len() {
                    (at, false)
                } else {
                    (0, true)
                }
            }
            (Direction::Backward, _) => {
                let at = self.matches.partition_point(|m| {
                    if inclusive {
                        m.line <= from
                    } else {
                        m.line < from
                    }
                });
                if at > 0 {
                    (at - 1, false)
                } else {
                    (self.matches.len() - 1, true)
                }
            }
        };
        self.current = Some(index);
        Some((self.matches[index], wrapped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "Alpha beta\nalpha\ngamma alpha\nDelta";

    fn search(pattern: &str) -> Search {
        Search::new(pattern, Direction::Forward, TEXT.lines())
    }

    #[test]
    fn smart_case() {
        assert_eq!(search("alpha").matches().len(), 3);
        assert_eq!(search("Alpha").matches().len(), 1);
    }

    #[test]
    fn regex_and_literal_fallback() {
        let s = search("a.p");
        assert_eq!(s.compiled, Compiled::Regex);
        assert_eq!(s.matches().len(), 3);
        let s = Search::new("(x", Direction::Forward, ["a(x)b"].into_iter());
        assert_eq!(s.compiled, Compiled::Literal);
        assert_eq!(
            s.matches(),
            &[Match {
                line: 0,
                start: 1,
                end: 3
            }]
        );
        assert!(search("").matches().is_empty());
        assert!(
            search("z*").matches().is_empty(),
            "empty matches are skipped"
        );
    }

    #[test]
    fn escapes_do_not_turn_off_smart_case() {
        let s = Search::new(r"\Wfoo", Direction::Forward, [" Foo"].into_iter());
        assert_eq!(s.matches().len(), 1);
        let s = Search::new(r"\p{Greek}x", Direction::Forward, ["αX"].into_iter());
        assert_eq!(s.matches().len(), 1);
        assert_eq!(
            search(r"\WAlpha").matches().len(),
            0,
            "a typed capital still counts"
        );
    }

    #[test]
    fn huge_patterns_and_match_counts_are_bounded() {
        // Too big even as plain text: no panic, no matches.
        let long = "ab".repeat(20_000);
        let s = Search::new(&long, Direction::Forward, [long.as_str()].into_iter());
        assert_eq!(s.compiled, Compiled::TooBig);
        assert!(s.matches().is_empty());
        assert_eq!(compile(r"\w{100}{100}").1, Compiled::TooBig);
        let text = "e".repeat(MAX_MATCHES + 10);
        let s = Search::new("e", Direction::Forward, [text.as_str()].into_iter());
        assert_eq!(s.matches().len(), MAX_MATCHES);
        assert!(s.capped);
    }

    #[test]
    fn a_preview_scans_from_the_view_within_its_budget() {
        let lines: Vec<String> = (0..100)
            .map(|i| format!("{i} {}", "x".repeat(PREVIEW_BYTES / 50)))
            .collect();
        let s = Search::preview(
            "7",
            Direction::Forward,
            (60..100).chain(0..60).map(|i| (i, lines[i].as_str())),
        );
        assert!(s.partial);
        let found: Vec<usize> = s.matches().iter().map(|m| m.line).collect();
        assert!(found.windows(2).all(|w| w[0] <= w[1]), "kept in order");
        assert!(found.contains(&70) && !found.contains(&17), "{found:?}");
    }

    #[test]
    fn steps_forward_and_back_with_wraparound() {
        let mut s = search("alpha");
        assert_eq!(s.step(0, Direction::Forward, true).unwrap().0.line, 0);
        assert_eq!(s.step(0, Direction::Forward, false).unwrap().0.line, 1);
        assert_eq!(s.step(0, Direction::Forward, false).unwrap().0.line, 2);
        let (m, wrapped) = s.step(0, Direction::Forward, false).unwrap();
        assert_eq!((m.line, wrapped), (0, true));
        let (m, wrapped) = s.step(0, Direction::Backward, false).unwrap();
        assert_eq!((m.line, wrapped), (2, true));
        assert_eq!(s.position(), Some((3, 3)));
    }

    #[test]
    fn first_step_starts_from_a_line() {
        let mut s = search("alpha");
        assert_eq!(s.step(1, Direction::Forward, false).unwrap().0.line, 2);
        let mut s = search("alpha");
        assert_eq!(s.step(2, Direction::Backward, false).unwrap().0.line, 1);
        let mut s = search("nothing");
        assert!(s.step(0, Direction::Forward, true).is_none());
    }
}
