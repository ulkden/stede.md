// SPDX-License-Identifier: GPL-3.0-or-later
//! The rendered pane's place, and keeping it in step with the raw pane.
//!
//! The front end lays the render model out (it knows its fonts and cells)
//! and reports each block's height in rows. The rendered pane's position is
//! a block and a row within it. Scroll sync maps between that and the raw
//! pane's line through each block's source range, proportionally inside a
//! block, so a long code block scrolls smoothly in both panes.

/// The rendered pane's layout (as reported) and where it is scrolled to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    /// Rows each block takes, for the document `generation` at `width`.
    heights: Vec<usize>,
    /// Whether any layout has been reported (an empty document has none to report).
    laid_out: bool,
    pub generation: u64,
    pub width: usize,
    block: usize,
    row: usize,
}

impl Rendered {
    /// Whether a layout has been reported for this document and width.
    pub fn is_current(&self, generation: u64, width: usize) -> bool {
        self.laid_out && self.generation == generation && self.width == width
    }

    /// A new layout. The place is kept (clamped); the caller syncs it after a
    /// new document.
    pub fn set_layout(
        &mut self,
        generation: u64,
        width: usize,
        heights: Vec<usize>,
        height: usize,
    ) {
        self.generation = generation;
        self.width = width;
        self.heights = heights;
        self.laid_out = true;
        self.clamp(height);
    }

    pub fn position(&self) -> (usize, usize) {
        (self.block, self.row)
    }

    fn rows_of(&self, block: usize) -> usize {
        self.heights.get(block).copied().unwrap_or(1).max(1)
    }

    /// The furthest place that still fills a pane `height` rows tall.
    fn max_position(&self, height: usize) -> (usize, usize) {
        let height = height.max(1);
        let mut below = 0;
        for b in (0..self.heights.len()).rev() {
            let r = self.rows_of(b);
            if below + r >= height {
                return (b, below + r - height);
            }
            below += r;
        }
        (0, 0)
    }

    fn clamp(&mut self, height: usize) {
        if self.heights.is_empty() {
            (self.block, self.row) = (0, 0);
            return;
        }
        let block = self.block.min(self.heights.len() - 1);
        let row = self.row.min(self.rows_of(block) - 1);
        (self.block, self.row) = (block, row).min(self.max_position(height));
    }

    /// Moves to a place, kept between the top and the last full page.
    pub fn set_position(&mut self, block: usize, row: usize, height: usize) {
        (self.block, self.row) = (block, row);
        self.clamp(height);
    }

    /// Whether it shows the last full page.
    pub fn at_end(&self, height: usize) -> bool {
        !self.heights.is_empty() && self.position() == self.max_position(height)
    }

    /// Goes to the end: the last full page.
    pub fn to_end(&mut self, height: usize) {
        (self.block, self.row) = self.max_position(height);
    }

    /// Scrolls by `delta` rows.
    pub fn scroll_rows(&mut self, delta: isize, height: usize) {
        if self.heights.is_empty() {
            return;
        }
        let (mut block, mut row) = (self.block, self.row);
        let mut left = delta.unsigned_abs();
        while left > 0 {
            if delta >= 0 {
                let here = self.rows_of(block);
                if row + 1 < here {
                    let step = left.min(here - 1 - row);
                    row += step;
                    left -= step;
                } else if block + 1 < self.heights.len() {
                    block += 1;
                    row = 0;
                    left -= 1;
                } else {
                    break;
                }
            } else if row > 0 {
                let step = left.min(row);
                row -= step;
                left -= step;
            } else if block > 0 {
                block -= 1;
                row = self.rows_of(block) - 1;
                left -= 1;
            } else {
                break;
            }
        }
        self.set_position(block, row, height);
    }

    /// Follows the raw pane: shows `block`, `into` of its `total` raw rows
    /// (wrapped rows of its source lines) along, at the same share of its
    /// rendered rows. Measuring in rows, not lines, keeps a block whose lines
    /// differ in length (one long line, one short) in step.
    pub fn follow(&mut self, block: usize, into: usize, total: usize, height: usize) {
        let row = into * self.rows_of(block) / total.max(1);
        self.set_position(block, row, height);
    }

    /// The share of the top block scrolled past: `(block, row, rows)`, for the
    /// raw pane to follow to the same share of that block's source.
    pub fn share(&self) -> (usize, usize, usize) {
        (self.block, self.row, self.rows_of(self.block))
    }

    /// The block shown at the top, for "jump to this block's source".
    pub fn top_block(&self) -> usize {
        self.block
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scrolls_by_rows_and_keeps_the_last_page_full() {
        let mut r = Rendered::default();
        r.set_layout(1, 40, vec![2, 10, 3], 4);
        r.scroll_rows(3, 4);
        assert_eq!(r.position(), (1, 1));
        r.scroll_rows(99, 4);
        assert_eq!(
            r.position(),
            (1, 9),
            "4 rows from the end: block 1's last row"
        );
        r.scroll_rows(-11, 4);
        assert_eq!(r.position(), (0, 0));
        r.to_end(4);
        assert_eq!(r.position(), (1, 9));
    }

    #[test]
    fn follows_by_share_of_rows() {
        let mut r = Rendered::default();
        r.set_layout(1, 40, vec![1, 20, 1], 4);
        // Half of block 1's raw rows: half of its 20 rendered rows.
        r.follow(1, 5, 10, 4);
        assert_eq!(r.position(), (1, 10));
        assert_eq!(r.share(), (1, 10, 20));
        r.follow(0, 0, 1, 4);
        assert_eq!(r.position(), (0, 0));
    }

    #[test]
    fn an_empty_layout_stays_put() {
        let mut r = Rendered::default();
        r.scroll_rows(5, 4);
        assert_eq!(r.position(), (0, 0));
        assert!(!r.is_current(0, 0));
    }
}
