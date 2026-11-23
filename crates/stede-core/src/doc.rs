// SPDX-License-Identifier: GPL-3.0-or-later
//! The open document: its text, render model, and where the raw pane is.
//!
//! The reader keeps the text as a `String` with the start of every line;
//! the rope buffer arrives with editing (phase 2).

use std::collections::HashMap;
use std::path::PathBuf;

use crate::io::Document;
use crate::render::{BlockKind, Container, RenderModel};

/// Byte offsets where each line starts. Lines end at `\n`, `\r\n` or a lone
/// `\r`, as comrak counts them, so line numbers agree with the render
/// model's source positions.
pub fn line_starts(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut starts = vec![0];
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => starts.push(i + 1),
            b'\r' => {
                if bytes.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                starts.push(i + 1);
            }
            _ => {}
        }
        i += 1;
    }
    // A final line ending doesn't start another (empty) line.
    if starts.len() > 1 && *starts.last().expect("at least one") == text.len() {
        starts.pop();
    }
    starts
}

/// How the raw pane colors a line: from the render model's block for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Plain,
    Heading(u8),
    Code,
    Quote,
    Rule,
    Table,
    /// Front matter and raw HTML: shown, but quiet.
    Meta,
}

/// The open document.
#[derive(Clone, Debug)]
pub struct Doc {
    pub path: PathBuf,
    pub text: String,
    pub model: RenderModel,
    /// Whether it was read as Markdown (else as one code block).
    pub markdown: bool,
    starts: Vec<usize>,
    /// The first line the raw pane shows, 0-based, and how many of its
    /// wrapped rows are scrolled past: a line taller than the pane can be
    /// read to its end.
    top: usize,
    top_row: usize,
    /// Rows each line wraps to, at the width they were counted for, so a
    /// long line isn't scanned again on every key.
    rows: HashMap<usize, (usize, usize)>,
    /// Bumped on every load, so front ends know to redo cached layout.
    pub generation: u64,
}

impl Doc {
    pub fn new(document: Document, generation: u64) -> Self {
        let starts = line_starts(&document.text);
        Self {
            path: document.path,
            text: document.text,
            model: document.model,
            markdown: document.markdown,
            starts,
            top: 0,
            top_row: 0,
            rows: HashMap::new(),
            generation,
        }
    }

    /// Where the raw pane starts: a line (0-based) and a row within it.
    pub fn position(&self) -> (usize, usize) {
        (self.top, self.top_row)
    }

    /// Rows line `i` wraps to at `width`.
    pub fn rows(&mut self, i: usize, width: usize) -> usize {
        if let Some(&(w, n)) = self.rows.get(&i)
            && w == width
        {
            return n;
        }
        let n = self.line(i).map_or(1, |l| crate::text::row_count(l, width));
        self.rows.insert(i, (width, n));
        n
    }

    /// The furthest position that still fills a pane `height` rows tall, so
    /// the last page stays full (Vim's `G` keeps the last line at the bottom).
    pub fn max_position(&mut self, width: usize, height: usize) -> (usize, usize) {
        let height = height.max(1);
        let mut below = 0;
        for i in (0..self.line_count()).rev() {
            let r = self.rows(i, width);
            if below + r >= height {
                return (i, below + r - height);
            }
            below += r;
        }
        (0, 0)
    }

    /// Moves to a position, kept between the top and the last full page.
    pub fn set_position(&mut self, line: usize, row: usize, width: usize, height: usize) {
        let line = line.min(self.line_count().saturating_sub(1));
        let row = row.min(self.rows(line, width).saturating_sub(1));
        let max = self.max_position(width, height);
        (self.top, self.top_row) = (line, row).min(max);
    }

    /// Scrolls by `delta` wrapped rows.
    pub fn scroll_rows(&mut self, delta: isize, width: usize, height: usize) {
        let (mut line, mut row) = (self.top, self.top_row);
        if delta >= 0 {
            let mut left = delta.unsigned_abs();
            while left > 0 {
                let here = self.rows(line, width);
                if row + 1 < here {
                    // Move within the line, as far as this step allows.
                    let step = left.min(here - 1 - row);
                    row += step;
                    left -= step;
                } else if line + 1 < self.line_count() {
                    line += 1;
                    row = 0;
                    left -= 1;
                } else {
                    break;
                }
            }
        } else {
            let mut left = delta.unsigned_abs();
            while left > 0 {
                if row > 0 {
                    let step = left.min(row);
                    row -= step;
                    left -= step;
                } else if line > 0 {
                    line -= 1;
                    row = self.rows(line, width) - 1;
                    left -= 1;
                } else {
                    break;
                }
            }
        }
        self.set_position(line, row, width, height);
    }

    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// Line `i` (0-based), without its line ending.
    pub fn line(&self, i: usize) -> Option<&str> {
        let start = *self.starts.get(i)?;
        let end = self.starts.get(i + 1).copied().unwrap_or(self.text.len());
        let line = &self.text[start..end];
        Some(line.trim_end_matches(['\n', '\r']))
    }

    /// Scrolls so `top` is the first line, kept in the document.
    pub fn scroll_to(&mut self, top: usize) {
        self.top = top.min(self.line_count().saturating_sub(1));
        self.top_row = 0;
    }

    /// How line `i` (0-based) is colored.
    pub fn line_kind(&self, i: usize) -> LineKind {
        let line = i + 1;
        let blocks = &self.model.blocks;
        let at = blocks.partition_point(|b| b.source.end < line);
        let Some(block) = blocks.get(at).filter(|b| b.source.contains(line)) else {
            return LineKind::Plain;
        };
        match &block.kind {
            BlockKind::Heading { level, .. } => LineKind::Heading(*level),
            BlockKind::Code { .. } => LineKind::Code,
            BlockKind::Rule => LineKind::Rule,
            BlockKind::Table(_) => LineKind::Table,
            BlockKind::FrontMatter(_) | BlockKind::Html(_) => LineKind::Meta,
            BlockKind::Paragraph(_) => {
                if block
                    .containers
                    .iter()
                    .any(|c| matches!(c, Container::Quote | Container::Alert { .. }))
                {
                    LineKind::Quote
                } else {
                    LineKind::Plain
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{Block, LineRange};

    #[test]
    fn line_starts_follow_every_line_ending() {
        assert_eq!(line_starts(""), vec![0]);
        assert_eq!(line_starts("a"), vec![0]);
        assert_eq!(line_starts("a\n"), vec![0]);
        assert_eq!(line_starts("a\nb"), vec![0, 2]);
        assert_eq!(line_starts("a\r\nb\rc\n\n"), vec![0, 3, 5, 7]);
    }

    fn doc(text: &str, blocks: Vec<Block>) -> Doc {
        Doc::new(
            Document {
                path: "/d.md".into(),
                text: text.into(),
                model: RenderModel { blocks },
                markdown: true,
            },
            1,
        )
    }

    #[test]
    fn lines_without_endings() {
        let d = doc("# T\r\nbody\n\nend", vec![]);
        assert_eq!(d.line_count(), 4);
        assert_eq!(d.line(0), Some("# T"));
        assert_eq!(d.line(2), Some(""));
        assert_eq!(d.line(3), Some("end"));
        assert_eq!(d.line(4), None);
    }

    #[test]
    fn line_kinds_come_from_blocks() {
        let block = |kind, start, end, containers| Block {
            kind,
            source: LineRange { start, end },
            containers,
        };
        let d = doc(
            "# T\n\n> q\n\n```\nx\n```\n",
            vec![
                block(
                    BlockKind::Heading {
                        level: 1,
                        text: vec![],
                    },
                    1,
                    1,
                    vec![],
                ),
                block(BlockKind::Paragraph(vec![]), 3, 3, vec![Container::Quote]),
                block(
                    BlockKind::Code {
                        lang: None,
                        text: "x".into(),
                    },
                    5,
                    7,
                    vec![],
                ),
            ],
        );
        assert_eq!(d.line_kind(0), LineKind::Heading(1));
        assert_eq!(d.line_kind(1), LineKind::Plain);
        assert_eq!(d.line_kind(2), LineKind::Quote);
        assert_eq!(d.line_kind(5), LineKind::Code);
    }

    #[test]
    fn scrolling_stays_in_the_document() {
        let mut d = doc("a\nb\nc", vec![]);
        d.scroll_to(99);
        assert_eq!(d.position(), (2, 0));
    }

    #[test]
    fn scrolls_by_rows_through_a_line_taller_than_the_pane() {
        // One 50-cell line at width 10 is 5 rows; the pane shows 2.
        let mut d = doc(&format!("{}\nend", "x".repeat(50)), vec![]);
        d.scroll_rows(1, 10, 2);
        assert_eq!(d.position(), (0, 1));
        d.scroll_rows(3, 10, 2);
        assert_eq!(
            d.position(),
            (0, 4),
            "the line's last row, then `end` below it"
        );
        d.scroll_rows(9, 10, 2);
        assert_eq!(d.position(), (0, 4), "the last page stays full");
        d.scroll_rows(-2, 10, 2);
        assert_eq!(d.position(), (0, 2));
    }

    #[test]
    fn the_last_page_stays_full() {
        let text: String = (1..=10).map(|i| format!("{i}\n")).collect();
        let mut d = doc(&text, vec![]);
        assert_eq!(d.max_position(80, 4), (6, 0));
        d.set_position(99, 0, 80, 4);
        assert_eq!(d.position(), (6, 0));
        // A file shorter than the pane never scrolls.
        let mut short = doc("a\nb", vec![]);
        assert_eq!(short.max_position(80, 4), (0, 0));
    }
}
