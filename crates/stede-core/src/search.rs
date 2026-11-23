// SPDX-License-Identifier: GPL-3.0-or-later
//! Search: a pattern, its matches in the document, and which one is current.
//!
//! Patterns are regular expressions with smart case: no capital letter means
//! case doesn't matter. A pattern that isn't a valid regex is searched as
//! plain text, so a stray `(` still finds itself.

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
    /// Whether the pattern wasn't a valid regex and is searched as text.
    pub literal: bool,
    matches: Vec<Match>,
    current: Option<usize>,
}

/// Builds the regex for a pattern: smart case, and plain text if the
/// pattern doesn't parse. Returns the regex and whether it fell back.
pub fn compile(pattern: &str) -> (Regex, bool) {
    let ignore_case = !pattern.chars().any(char::is_uppercase);
    let build = |p: &str| {
        RegexBuilder::new(p)
            .case_insensitive(ignore_case)
            // A pattern typed by hand shouldn't build a huge automaton.
            .size_limit(1 << 20)
            .build()
    };
    match build(pattern) {
        Ok(regex) => (regex, false),
        Err(_) => (
            build(&regex::escape(pattern)).expect("an escaped pattern is valid"),
            true,
        ),
    }
}

impl Search {
    /// A search over `lines`. Empty patterns match nothing.
    pub fn new<'a>(
        pattern: &str,
        direction: Direction,
        lines: impl Iterator<Item = &'a str>,
    ) -> Self {
        let (regex, literal) = compile(pattern);
        let mut search = Self {
            pattern: pattern.to_owned(),
            direction,
            regex,
            literal,
            matches: Vec::new(),
            current: None,
        };
        search.rescan(lines);
        search
    }

    /// Finds the matches again (the document changed).
    pub fn rescan<'a>(&mut self, lines: impl Iterator<Item = &'a str>) {
        self.matches.clear();
        self.current = None;
        if self.pattern.is_empty() {
            return;
        }
        for (i, line) in lines.enumerate() {
            for m in self.regex.find_iter(line) {
                // An empty match (`a*`) would light nothing; skip it.
                if m.start() < m.end() {
                    self.matches.push(Match {
                        line: i,
                        start: m.start(),
                        end: m.end(),
                    });
                }
            }
        }
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
        assert!(!s.literal);
        assert_eq!(s.matches().len(), 3);
        let s = Search::new("(x", Direction::Forward, ["a(x)b"].into_iter());
        assert!(s.literal);
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
