// SPDX-License-Identifier: GPL-3.0-or-later
//! The open document: its text, render model, and where the raw pane is.
//!
//! The reader keeps the text as a `String` with the start of every line;
//! the rope buffer arrives with editing (phase 2).

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
    /// The first line the raw pane shows, 0-based.
    pub top: usize,
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
            generation,
        }
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
        assert_eq!(d.top, 2);
    }
}
