// SPDX-License-Identifier: GPL-3.0-or-later
//! The render model: what the rendered pane draws, with no UI types.
//!
//! It lives in the core so every part shares it: `stede-markdown` builds it,
//! the services carry it back from a worker, and front ends draw it.
//!
//! A document is a flat list of leaf [`Block`]s in reading order. Containers
//! (block quotes, alerts, list items, footnotes) aren't blocks of their own;
//! each leaf block lists the containers it sits in, outermost first, so a
//! front end draws their bars, markers and indents as a prefix. Every block
//! carries the source lines it came from, which is what keeps the raw and
//! rendered panes scrolled together.

/// Lines in the source, 1-based and inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LineRange {
    pub start: usize,
    pub end: usize,
}

impl LineRange {
    pub fn contains(&self, line: usize) -> bool {
        (self.start..=self.end).contains(&line)
    }
}

/// How a run of inline text is styled. A front end maps these to its own
/// look (the theme decides colors).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SpanStyle {
    pub emph: bool,
    pub strong: bool,
    pub strike: bool,
    pub underline: bool,
    pub code: bool,
    pub link: bool,
    /// Alt text of an image.
    pub image: bool,
    /// A footnote reference such as `[^1]`.
    pub footnote_ref: bool,
    /// Raw inline HTML, shown as written.
    pub html: bool,
}

/// A run of text in one style. `\n` in `text` is a hard line break.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: SpanStyle,
    /// Where a link goes; for a footnote reference, the footnote's name. An
    /// image inside a link (a README badge) keeps the link's target here and
    /// its own source in `image`.
    pub target: Option<String>,
    /// The source of an image; the span's text is its alt text.
    pub image: Option<String>,
}

/// One table cell: its inline content.
pub type Cell = Vec<Span>;

/// Column alignment in a table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

/// A GFM table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub align: Vec<Align>,
    pub header: Vec<Cell>,
    pub rows: Vec<Vec<Cell>>,
}

/// The kind of a GFM alert (`> [!NOTE]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

/// A list item's marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marker {
    /// `-`, `*` or `+`.
    Bullet(char),
    /// `1.` or `1)`.
    Ordered { number: usize, delimiter: char },
}

/// A container a leaf block sits in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Container {
    Quote,
    /// `continuation` is false on the alert's first block (draw its label)
    /// and true after, so two alerts side by side each get their label.
    Alert {
        kind: AlertKind,
        title: Option<String>,
        continuation: bool,
    },
    /// `continuation` is false on the item's first block (draw the marker or
    /// task box) and true on the blocks after it (draw only the indent).
    ListItem {
        marker: Marker,
        task: Option<bool>,
        continuation: bool,
    },
    Footnote {
        name: String,
        continuation: bool,
    },
}

/// What a leaf block is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Heading {
        level: u8,
        text: Vec<Span>,
    },
    Paragraph(Vec<Span>),
    /// A code block; `lang` is the first word of the info string.
    Code {
        lang: Option<String>,
        text: String,
    },
    /// A raw HTML block, shown as written.
    Html(String),
    /// A thematic break (`---`).
    Rule,
    Table(Table),
    /// YAML front matter, without its `---` fences.
    FrontMatter(String),
}

/// One leaf block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub source: LineRange,
    /// Containers around this block, outermost first.
    pub containers: Vec<Container>,
}

/// A rendered document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenderModel {
    pub blocks: Vec<Block>,
}

impl RenderModel {
    /// The block to show for a source line: the one that holds it, else the
    /// next block after it, else the last block. `None` for an empty document.
    ///
    /// Blocks are in reading order with non-decreasing start lines, so this
    /// is a binary search.
    pub fn block_at_line(&self, line: usize) -> Option<usize> {
        if self.blocks.is_empty() {
            return None;
        }
        // The first block that ends at or after `line`.
        let i = self.blocks.partition_point(|b| b.source.end < line);
        Some(i.min(self.blocks.len() - 1))
    }

    /// The source line a block starts on: where the raw pane goes when a
    /// rendered block is chosen.
    pub fn line_of_block(&self, index: usize) -> Option<usize> {
        self.blocks.get(index).map(|b| b.source.start)
    }
}
