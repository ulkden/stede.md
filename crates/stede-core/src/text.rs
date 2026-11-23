// SPDX-License-Identifier: GPL-3.0-or-later
//! Wrapping lines into rows of cells, by display width.
//!
//! It lives in the core so scrolling (which moves by rows) and drawing agree
//! on where every row breaks. Wide characters (CJK, emoji) take two cells,
//! tabs stop every [`TAB_STOP`] columns of the line, and control characters
//! show as `�` so they can't move the terminal's cursor.

use unicode_width::UnicodeWidthChar;

/// Columns a tab advances to (a multiple of this).
pub const TAB_STOP: usize = 4;

/// The rows from `skip` up to `skip + take` of `line` wrapped to `width`
/// cells. Only those rows are built, and the scan stops after the last one,
/// so a huge single line costs only as far as the rows asked for.
pub fn wrap_rows(line: &str, width: usize, skip: usize, take: usize) -> Vec<String> {
    let width = width.max(1);
    let end = skip.saturating_add(take);
    let mut rows = Vec::new();
    let mut row = 0;
    let mut current = String::new();
    let mut col = 0;
    // The column in the whole line, which tab stops count from (rows wrap).
    let mut line_col = 0;
    let mut place =
        |shown: char, w: usize, row: &mut usize, col: &mut usize, current: &mut String| -> bool {
            if *col + w > width && *col > 0 {
                if *row >= skip {
                    rows.push(std::mem::take(current));
                }
                current.clear();
                *row += 1;
                *col = 0;
                if *row >= end {
                    return false;
                }
            }
            if *row >= skip {
                current.push(shown);
            }
            *col += w;
            true
        };
    for c in line.chars() {
        if c == '\t' {
            let spaces = TAB_STOP - line_col % TAB_STOP;
            line_col += spaces;
            for _ in 0..spaces {
                if !place(' ', 1, &mut row, &mut col, &mut current) {
                    return rows;
                }
            }
            continue;
        }
        let shown = if c.is_control() { '\u{FFFD}' } else { c };
        let w = UnicodeWidthChar::width(shown).unwrap_or(0);
        line_col += w;
        if !place(shown, w, &mut row, &mut col, &mut current) {
            return rows;
        }
    }
    if row >= skip && row < end {
        rows.push(current);
    }
    rows
}

/// Every row of `line` wrapped to `width`. At least one: an empty line takes a row.
pub fn wrap(line: &str, width: usize) -> Vec<String> {
    wrap_rows(line, width, 0, usize::MAX)
}

/// How many rows `line` takes at `width`, without building them.
pub fn row_count(line: &str, width: usize) -> usize {
    let width = width.max(1);
    let mut rows = 1;
    let mut col = 0;
    let mut line_col = 0;
    let mut place = |w: usize, col: &mut usize| {
        if *col + w > width && *col > 0 {
            rows += 1;
            *col = 0;
        }
        *col += w;
    };
    for c in line.chars() {
        if c == '\t' {
            let spaces = TAB_STOP - line_col % TAB_STOP;
            line_col += spaces;
            for _ in 0..spaces {
                place(1, &mut col);
            }
            continue;
        }
        let shown = if c.is_control() { '\u{FFFD}' } else { c };
        let w = UnicodeWidthChar::width(shown).unwrap_or(0);
        line_col += w;
        place(w, &mut col);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_by_display_width_with_tabs_and_controls() {
        assert_eq!(wrap("", 5), vec![""]);
        assert_eq!(wrap("abcdefg", 3), vec!["abc", "def", "g"]);
        // A wide character that doesn't fit moves to the next row whole.
        assert_eq!(wrap("ab日本", 3), vec!["ab", "日", "本"]);
        assert_eq!(wrap("\tx", 8), vec!["    x"]);
        assert_eq!(wrap("a\u{7}b", 8), vec!["a\u{FFFD}b"]);
        // Tab stops count from the start of the line, not of the row.
        assert_eq!(wrap("abcdefg\tx", 6), vec!["abcdef", "g x"]);
    }

    #[test]
    fn a_window_of_rows_and_counts_agree() {
        let line = "the quick brown fox jumps over the lazy dog";
        let all = wrap(line, 7);
        assert_eq!(row_count(line, 7), all.len());
        assert_eq!(wrap_rows(line, 7, 2, 3), all[2..5].to_vec());
        assert_eq!(wrap_rows(line, 7, 5, 99), all[5..].to_vec());
        assert!(wrap_rows(line, 7, 99, 3).is_empty());
        for (l, w) in [("", 3), ("ab日本", 3), ("\t\tx", 5), ("abcdefg\tx", 6)] {
            assert_eq!(row_count(l, w), wrap(l, w).len(), "{l:?} at {w}");
        }
    }

    #[test]
    fn a_huge_line_stops_early() {
        let line = "x".repeat(10_000_000);
        let start = std::time::Instant::now();
        assert_eq!(wrap_rows(&line, 80, 0, 5).len(), 5);
        assert!(start.elapsed() < std::time::Duration::from_millis(50));
    }
}
