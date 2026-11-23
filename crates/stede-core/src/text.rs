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
    wrap_rows_mapped(line, width, skip, take)
        .into_iter()
        .map(|(row, _)| row)
        .collect()
}

/// [`wrap_rows`], with the byte offset in `line` of each character of each
/// row (a tab's spaces all point at the tab), so matches found in the
/// source line can be lit where they're drawn.
pub fn wrap_rows_mapped(
    line: &str,
    width: usize,
    skip: usize,
    take: usize,
) -> Vec<(String, Vec<usize>)> {
    let width = width.max(1);
    let end = skip.saturating_add(take);
    let mut rows = Vec::new();
    let mut row = 0;
    let mut current = (String::new(), Vec::new());
    let mut col = 0;
    // The column in the whole line, which tab stops count from (rows wrap).
    let mut line_col = 0;
    let mut place = |shown: char,
                     at: usize,
                     w: usize,
                     row: &mut usize,
                     col: &mut usize,
                     current: &mut (String, Vec<usize>)|
     -> bool {
        if *col + w > width && *col > 0 {
            let full = std::mem::take(current);
            if *row >= skip {
                rows.push(full);
            }
            *row += 1;
            *col = 0;
            if *row >= end {
                return false;
            }
        }
        if *row >= skip {
            current.0.push(shown);
            current.1.push(at);
        }
        *col += w;
        true
    };
    for (at, c) in line.char_indices() {
        if c == '\t' {
            let spaces = TAB_STOP - line_col % TAB_STOP;
            line_col += spaces;
            for _ in 0..spaces {
                if !place(' ', at, 1, &mut row, &mut col, &mut current) {
                    return rows;
                }
            }
            continue;
        }
        let shown = if c.is_control() { '\u{FFFD}' } else { c };
        let w = UnicodeWidthChar::width(shown).unwrap_or(0);
        line_col += w;
        if !place(shown, at, w, &mut row, &mut col, &mut current) {
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

/// The wrapped row (0-based) that the character at byte `at` of `line`
/// is drawn on, at `width`.
pub fn row_at(line: &str, width: usize, at: usize) -> usize {
    let at = at.min(line.len());
    let end = line[at..].chars().next().map_or(at, |c| at + c.len_utf8());
    row_count(&line[..end], width) - 1
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
    fn rows_map_back_to_source_bytes() {
        let rows = wrap_rows_mapped("a\tb日c", 4, 0, 9);
        assert_eq!(rows[0], ("a   ".to_owned(), vec![0, 1, 1, 1]));
        assert_eq!(rows[1], ("b日c".to_owned(), vec![2, 3, 6]));
        assert_eq!(row_at("abcdefg", 3, 0), 0);
        assert_eq!(row_at("abcdefg", 3, 3), 1);
        assert_eq!(row_at("abcdefg", 3, 6), 2);
        // A wide character that wraps whole is on the next row.
        assert_eq!(row_at("ab日", 3, 2), 1);
    }

    #[test]
    fn a_huge_line_stops_early() {
        let line = "x".repeat(10_000_000);
        let start = std::time::Instant::now();
        assert_eq!(wrap_rows(&line, 80, 0, 5).len(), 5);
        assert!(start.elapsed() < std::time::Duration::from_millis(50));
    }
}
