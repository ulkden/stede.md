// SPDX-License-Identifier: GPL-3.0-or-later
//! Lays the render model out for the terminal: wrapped, styled rows per block.
//!
//! The core decides what each block is; this decides how it looks in cells.
//! Each block's rows include the blank row that separates it from the one
//! before, so the heights reported to the core (for scrolling and sync) are
//! exactly what's drawn.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use stede_core::render::{
    AlertKind, Align, Block, BlockKind, Cell, Container, Marker, RenderModel, Span as MdSpan, Table,
};
use unicode_width::UnicodeWidthStr;

use crate::ui::Theme;

/// A laid-out document, for one generation, width and theme.
#[derive(Clone, Debug, Default)]
pub struct RenderCache {
    pub generation: u64,
    pub width: usize,
    pub theme_epoch: u64,
    /// Whether it holds a layout (an empty document's is empty but done).
    pub laid_out: bool,
    /// Each block's rows.
    pub blocks: Vec<Vec<Line<'static>>>,
}

impl RenderCache {
    /// Lays the model out again if the document, width or theme changed.
    /// Returns whether it did.
    pub fn update(
        &mut self,
        model: &RenderModel,
        generation: u64,
        width: usize,
        theme: &Theme,
        theme_epoch: u64,
    ) -> bool {
        if self.generation == generation
            && self.width == width
            && self.theme_epoch == theme_epoch
            && self.laid_out
        {
            return false;
        }
        self.blocks = layout(model, width, theme);
        self.generation = generation;
        self.width = width;
        self.theme_epoch = theme_epoch;
        self.laid_out = true;
        true
    }

    pub fn heights(&self) -> Vec<usize> {
        self.blocks.iter().map(Vec::len).collect()
    }
}

/// Lays every block out at `width` cells.
pub fn layout(model: &RenderModel, width: usize, theme: &Theme) -> Vec<Vec<Line<'static>>> {
    let width = width.max(1);
    let mut out = Vec::with_capacity(model.blocks.len());
    let mut previous: Option<&Block> = None;
    for block in &model.blocks {
        let mut rows = Vec::new();
        if let Some(prev) = previous
            && !tight(prev, block)
        {
            rows.push(Line::default());
        }
        // An alert's label goes above its first block. The label row takes
        // any list marker, so the block's own first row is an indent.
        // The label row takes the markers of the alert and what's around it
        // (a list item it sits in); containers inside the alert (a list it
        // opens with) mark the block's own first row.
        let mut label_upto = None;
        if let Some((
            at,
            Container::Alert {
                kind,
                title,
                continuation: false,
            },
        )) = block
            .containers
            .iter()
            .enumerate()
            .find(|(_, c)| matches!(c, Container::Alert { .. }))
        {
            label_upto = Some(at);
            let label = title
                .clone()
                .unwrap_or_else(|| alert_label(*kind).to_owned());
            let style = Style::new()
                .fg(alert_color(*kind, theme))
                .add_modifier(Modifier::BOLD);
            let (first, _) = prefixes(&block.containers[..=at], theme, true);
            let mut spans = first;
            spans.push(Span::styled(label, style));
            rows.push(Line::from(spans));
        }
        rows.extend(block_rows(block, width, theme, label_upto));
        out.push(rows);
        previous = Some(block);
    }
    out
}

/// Whether two blocks sit with no blank row between them: items of one tight
/// list, and blocks that continue the same list item.
fn tight(prev: &Block, next: &Block) -> bool {
    let item = |b: &Block| {
        b.containers
            .iter()
            .any(|c| matches!(c, Container::ListItem { .. }))
    };
    item(prev)
        && item(next)
        && matches!(prev.kind, BlockKind::Paragraph(_))
        && matches!(next.kind, BlockKind::Paragraph(_))
        && next.source.start <= prev.source.end + 1
}

fn alert_label(kind: AlertKind) -> &'static str {
    match kind {
        AlertKind::Note => "Note",
        AlertKind::Tip => "Tip",
        AlertKind::Important => "Important",
        AlertKind::Warning => "Warning",
        AlertKind::Caution => "Caution",
    }
}

fn alert_color(kind: AlertKind, theme: &Theme) -> ratatui::style::Color {
    match kind {
        AlertKind::Note => theme.accent,
        AlertKind::Tip => theme.code,
        AlertKind::Important => theme.quote,
        AlertKind::Warning | AlertKind::Caution => theme.heading,
    }
}

/// The prefix spans for a block's first row and for the rows after it.
fn prefixes(
    containers: &[Container],
    theme: &Theme,
    first: bool,
) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    let mut head = Vec::new();
    let mut rest = Vec::new();
    for c in containers {
        match c {
            Container::Quote => {
                let bar = Span::styled("│ ", Style::new().fg(theme.quote));
                head.push(bar.clone());
                rest.push(bar);
            }
            Container::Alert { kind, .. } => {
                let bar = Span::styled("│ ", Style::new().fg(alert_color(*kind, theme)));
                head.push(bar.clone());
                rest.push(bar);
            }
            Container::ListItem {
                marker,
                task,
                continuation,
            } => {
                let text = match (task, marker) {
                    (Some(true), _) => "☑ ".to_owned(),
                    (Some(false), _) => "☐ ".to_owned(),
                    (None, Marker::Bullet(_)) => "• ".to_owned(),
                    (None, Marker::Ordered { number, delimiter }) => {
                        format!("{number}{delimiter} ")
                    }
                };
                let w = text.width();
                if first && !continuation {
                    head.push(Span::styled(text, Style::new().fg(theme.accent)));
                } else {
                    head.push(Span::raw(" ".repeat(w)));
                }
                rest.push(Span::raw(" ".repeat(w)));
            }
            Container::Footnote { name, continuation } => {
                let text = format!("[^{name}] ");
                let w = text.width();
                if first && !continuation {
                    head.push(Span::styled(text, Style::new().fg(theme.dim)));
                } else {
                    head.push(Span::raw(" ".repeat(w)));
                }
                rest.push(Span::raw(" ".repeat(w)));
            }
        }
    }
    (head, rest)
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(Span::width).sum()
}

/// A block's rows, each with its container prefix.
/// `label_upto`: an alert label row already took the markers of containers
/// `..=label_upto`, so those show as indents on the first row.
fn block_rows(
    block: &Block,
    width: usize,
    theme: &Theme,
    label_upto: Option<usize>,
) -> Vec<Line<'static>> {
    let (head, rest) = prefixes(&block.containers, theme, true);
    let head = match label_upto {
        Some(at) => {
            let (_, outer) = prefixes(&block.containers[..=at], theme, true);
            let (inner, _) = prefixes(&block.containers[at + 1..], theme, true);
            outer.into_iter().chain(inner).collect()
        }
        None => head,
    };
    let prefix_width = spans_width(&head).max(spans_width(&rest));
    // Very deep nesting leaves at least a few cells for the content.
    let inner = width.saturating_sub(prefix_width).max(width.min(4)).max(1);
    let content = content_rows(block, inner, theme);
    let mut rows = Vec::with_capacity(content.len());
    for (i, row) in content.into_iter().enumerate() {
        let mut spans = if i == 0 { head.clone() } else { rest.clone() };
        spans.extend(row);
        rows.push(Line::from(spans));
    }
    rows
}

/// The rows of a block's content, without prefixes, at `width` cells.
fn content_rows(block: &Block, width: usize, theme: &Theme) -> Vec<Vec<Span<'static>>> {
    match &block.kind {
        BlockKind::Heading { level, text } => {
            let base = Style::new().fg(theme.heading).add_modifier(Modifier::BOLD);
            let base = if *level >= 3 {
                base
            } else {
                base.add_modifier(Modifier::UNDERLINED)
            };
            let mut rows = wrap_spans(&styled(text, base, theme), width);
            if *level <= 2 {
                let w = rows
                    .iter()
                    .map(|r| spans_width(r))
                    .max()
                    .unwrap_or(0)
                    .max(1)
                    .min(width);
                let line = if *level == 1 { "━" } else { "─" };
                rows.push(vec![Span::styled(
                    line.repeat(w),
                    Style::new().fg(theme.heading),
                )]);
            }
            rows
        }
        BlockKind::Paragraph(text) => wrap_spans(&styled(text, Style::new(), theme), width),
        BlockKind::Code { lang, text } => {
            let style = Style::new().fg(theme.code).bg(theme.code_bg);
            let mut rows = Vec::new();
            if let Some(lang) = lang {
                rows.push(vec![Span::styled(
                    pad(lang, width),
                    Style::new().fg(theme.dim).bg(theme.code_bg),
                )]);
            }
            for line in text.split('\n') {
                for row in stede_core::text::wrap(line, width) {
                    rows.push(vec![Span::styled(pad(&row, width), style)]);
                }
            }
            rows
        }
        BlockKind::Html(text) | BlockKind::FrontMatter(text) => {
            let style = Style::new().fg(theme.meta);
            text.split('\n')
                .flat_map(|l| stede_core::text::wrap(l, width))
                .map(|r| vec![Span::styled(r, style)])
                .collect()
        }
        BlockKind::Rule => vec![vec![Span::styled(
            "─".repeat(width),
            Style::new().fg(theme.rule),
        )]],
        BlockKind::Table(table) => table_rows(table, width, theme),
    }
}

/// A row padded with spaces to `width` cells (for the code band's color).
fn pad(text: &str, width: usize) -> String {
    let w = text.width();
    format!("{text}{}", " ".repeat(width.saturating_sub(w)))
}

/// Markdown spans as styled text pieces.
fn styled(spans: &[MdSpan], base: Style, theme: &Theme) -> Vec<(String, Style)> {
    spans
        .iter()
        .map(|s| {
            let st = s.style;
            let mut style = base;
            if st.emph {
                style = style.add_modifier(Modifier::ITALIC);
            }
            if st.strong {
                style = style.add_modifier(Modifier::BOLD);
            }
            if st.strike {
                style = style.add_modifier(Modifier::CROSSED_OUT);
            }
            if st.underline {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            if st.code {
                style = style.fg(theme.code);
            }
            if st.html || st.footnote_ref {
                style = style.fg(theme.dim);
            }
            if st.link {
                style = style.fg(theme.accent).add_modifier(Modifier::UNDERLINED);
            }
            // Tabs as spaces, so cells are measured as they're drawn.
            let text = if st.image {
                style = style.fg(theme.dim).add_modifier(Modifier::ITALIC);
                format!("▣ {}", s.text.replace('\t', "    "))
            } else {
                s.text.replace('\t', "    ")
            };
            (text, style)
        })
        .collect()
}

/// Word-wraps styled text to `width` cells. `\n` is a hard break. A word
/// wider than a whole row is split by characters. Always at least one row.
fn wrap_spans(pieces: &[(String, Style)], width: usize) -> Vec<Vec<Span<'static>>> {
    let width = width.max(1);
    // Words and the spaces between them, each with its style.
    let mut tokens: Vec<(String, Style, bool)> = Vec::new(); // (text, style, is_space)
    let mut breaks: Vec<usize> = Vec::new(); // token indexes where a hard break goes
    for (text, style) in pieces {
        for (i, part) in text.split('\n').enumerate() {
            if i > 0 {
                breaks.push(tokens.len());
            }
            let mut word = String::new();
            for c in part.chars() {
                let shown = if c.is_control() { '\u{FFFD}' } else { c };
                if shown == ' ' {
                    if !word.is_empty() {
                        tokens.push((std::mem::take(&mut word), *style, false));
                    }
                    tokens.push((" ".into(), *style, true));
                } else {
                    word.push(shown);
                }
            }
            if !word.is_empty() {
                tokens.push((word, *style, false));
            }
        }
    }
    let mut rows: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut col = 0;
    let mut next_break = 0;
    for (i, (text, style, space)) in tokens.into_iter().enumerate() {
        while next_break < breaks.len() && breaks[next_break] == i {
            rows.push(Vec::new());
            col = 0;
            next_break += 1;
        }
        let w = text.width();
        if space {
            // A space at a row's end or start is dropped.
            if col > 0 && col < width {
                rows.last_mut()
                    .expect("a row")
                    .push(Span::styled(text, style));
                col += 1;
            }
            continue;
        }
        if col + w > width && col > 0 {
            // Trim a trailing space before wrapping.
            if let Some(last) = rows.last_mut()
                && last.last().is_some_and(|s| s.content == " ")
            {
                last.pop();
            }
            rows.push(Vec::new());
            col = 0;
        }
        if w > width {
            // A word longer than a row: split it by characters.
            for piece in stede_core::text::wrap(&text, width) {
                if col > 0 {
                    rows.push(Vec::new());
                }
                col = piece.width();
                rows.last_mut()
                    .expect("a row")
                    .push(Span::styled(piece, style));
            }
            continue;
        }
        rows.last_mut()
            .expect("a row")
            .push(Span::styled(text, style));
        col += w;
    }
    // Hard breaks at the very end still make their rows.
    while next_break < breaks.len() {
        rows.push(Vec::new());
        next_break += 1;
    }
    rows
}

/// A table with box borders, columns shrunk to fit and cells wrapped. Too
/// narrow for borders, it falls back to plain rows of `a | b | c`.
fn table_rows(table: &Table, width: usize, theme: &Theme) -> Vec<Vec<Span<'static>>> {
    let columns = table
        .header
        .len()
        .max(table.rows.iter().map(Vec::len).max().unwrap_or(0))
        .max(1);
    let border = Style::new().fg(theme.rule);
    // Borders: one per column plus one; padding: one cell each side.
    let overhead = columns + 1 + columns * 2;
    if width < overhead + columns {
        return table_fallback(table, width, theme);
    }
    let text = |cell: &Cell| styled(cell, Style::new(), theme);
    let natural: Vec<usize> = (0..columns)
        .map(|c| {
            table
                .header
                .get(c)
                .into_iter()
                .chain(table.rows.iter().filter_map(|r| r.get(c)))
                .map(|cell| text(cell).iter().map(|(t, _)| t.width()).sum::<usize>())
                .max()
                .unwrap_or(0)
                .max(1)
        })
        .collect();
    // A column can't be narrower than its widest character (CJK and emoji
    // take two cells), or its borders would shift.
    let minimum: Vec<usize> = (0..columns)
        .map(|c| {
            table
                .header
                .get(c)
                .into_iter()
                .chain(table.rows.iter().filter_map(|r| r.get(c)))
                .flat_map(&text)
                .flat_map(|(t, _)| {
                    t.chars()
                        .map(|ch| unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0))
                        .collect::<Vec<_>>()
                })
                .max()
                .unwrap_or(1)
                .max(1)
        })
        .collect();
    if minimum.iter().sum::<usize>() > width - overhead {
        return table_fallback(table, width, theme);
    }
    let widths: Vec<usize> = fit_columns(&natural, width - overhead)
        .into_iter()
        .zip(&minimum)
        .map(|(w, &m)| w.max(m))
        .collect();
    if widths.iter().sum::<usize>() > width - overhead {
        return table_fallback(table, width, theme);
    }
    let align = |c: usize| table.align.get(c).copied().unwrap_or(Align::None);
    let rule = |l: &str, m: &str, r: &str| {
        let mut s = String::from(l);
        for (i, w) in widths.iter().enumerate() {
            s.push_str(&"─".repeat(w + 2));
            s.push_str(if i + 1 < columns { m } else { r });
        }
        vec![Span::styled(s, border)]
    };
    let draw_row = |cells: &[Cell], header: bool| -> Vec<Vec<Span<'static>>> {
        let wrapped: Vec<Vec<Vec<Span<'static>>>> = (0..columns)
            .map(|c| {
                let mut pieces = cells.get(c).map(text).unwrap_or_default();
                if header {
                    for p in &mut pieces {
                        p.1 = p.1.add_modifier(Modifier::BOLD);
                    }
                }
                wrap_spans(&pieces, widths[c])
            })
            .collect();
        let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
        (0..height)
            .map(|r| {
                let mut spans = vec![Span::styled("│", border)];
                for (c, cell) in wrapped.iter().enumerate() {
                    let content = cell.get(r).cloned().unwrap_or_default();
                    let used = spans_width(&content);
                    let free = widths[c].saturating_sub(used);
                    let (left, right) = match align(c) {
                        Align::Right => (free, 0),
                        Align::Center => (free / 2, free - free / 2),
                        Align::Left | Align::None => (0, free),
                    };
                    spans.push(Span::raw(" ".repeat(left + 1)));
                    spans.extend(content);
                    spans.push(Span::raw(" ".repeat(right + 1)));
                    spans.push(Span::styled("│", border));
                }
                spans
            })
            .collect()
    };
    let mut rows = vec![rule("┌", "┬", "┐")];
    rows.extend(draw_row(&table.header, true));
    rows.push(rule("├", "┼", "┤"));
    for row in &table.rows {
        rows.extend(draw_row(row, false));
    }
    rows.push(rule("└", "┴", "┘"));
    rows
}

/// Column widths summing to at most `available`: natural widths if they fit,
/// else the widest columns shrink first, none below a share of the space.
fn fit_columns(natural: &[usize], available: usize) -> Vec<usize> {
    let mut widths = natural.to_vec();
    let floor = (available / natural.len().max(1)).max(1);
    while widths.iter().sum::<usize>() > available {
        let (i, &w) = widths
            .iter()
            .enumerate()
            .max_by_key(|(_, w)| **w)
            .expect("at least one column");
        if w <= floor {
            // Nothing left to shrink fairly: cap every column at the floor.
            for w in &mut widths {
                *w = (*w).min(floor);
            }
            break;
        }
        widths[i] -= 1;
    }
    widths
}

fn table_fallback(table: &Table, width: usize, theme: &Theme) -> Vec<Vec<Span<'static>>> {
    let mut rows = Vec::new();
    for (i, cells) in std::iter::once(&table.header)
        .chain(&table.rows)
        .enumerate()
    {
        let mut pieces = Vec::new();
        for (c, cell) in cells.iter().enumerate() {
            if c > 0 {
                pieces.push((" | ".to_owned(), Style::new().fg(theme.rule)));
            }
            let mut text = styled(cell, Style::new(), theme);
            if i == 0 {
                for p in &mut text {
                    p.1 = p.1.add_modifier(Modifier::BOLD);
                }
            }
            pieces.extend(text);
        }
        rows.extend(wrap_spans(&pieces, width));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use stede_markdown::{Flavor, render};

    fn text_of(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    fn laid_out(md: &str, width: usize) -> Vec<String> {
        layout(&render(md, Flavor::Gfm), width, &Theme::default())
            .into_iter()
            .flat_map(|rows| text_of(&rows))
            .collect()
    }

    #[test]
    fn word_wraps_prose_and_keeps_hard_breaks() {
        assert_eq!(
            laid_out("one two three four five\n", 9),
            vec!["one two", "three", "four five"]
        );
        assert_eq!(laid_out("a\\\nb\n", 9), vec!["a", "b"]);
        // A word wider than the row splits by characters.
        assert_eq!(laid_out("abcdefghij\n", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn lists_tasks_quotes_and_alerts() {
        assert_eq!(
            laid_out(
                "- one\n- [x] done\n  more\n\n> quoted\n\n> [!NOTE]\n> careful\n",
                20
            ),
            vec![
                "• one",
                "☑ done more",
                "",
                "│ quoted",
                "",
                "│ Note",
                "│ careful"
            ]
        );
    }

    #[test]
    fn side_by_side_alerts_each_get_their_label_and_tabs_expand() {
        assert_eq!(
            laid_out("> [!NOTE]\n> a\n\n> [!WARNING]\n> b\n", 20),
            vec!["│ Note", "│ a", "", "│ Warning", "│ b"]
        );
        assert_eq!(laid_out("a\tb `x\ty`\n", 20), vec!["a    b x    y"]);
    }

    #[test]
    fn an_alert_opening_with_a_list_keeps_the_bullet_on_the_item() {
        assert_eq!(
            laid_out("> [!NOTE]\n> - first\n> - second\n", 20),
            vec!["│ Note", "│ • first", "│ • second"]
        );
    }

    #[test]
    fn wide_characters_keep_table_borders_straight() {
        use unicode_width::UnicodeWidthStr;
        let rows = laid_out("| 日本 | x | y |\n|---|---|---|\n| 語 | a | b |\n", 16);
        let widths: Vec<usize> = rows.iter().map(|r| r.width()).collect();
        assert!(widths.windows(2).all(|w| w[0] == w[1]), "{rows:#?}");
    }

    #[test]
    fn tables_fit_and_fall_back() {
        let md = "| Left | Right |\n|:-----|------:|\n| a | 1 |\n";
        assert_eq!(
            laid_out(md, 30),
            vec![
                "┌──────┬───────┐",
                "│ Left │ Right │",
                "├──────┼───────┤",
                "│ a    │     1 │",
                "└──────┴───────┘",
            ]
        );
        assert_eq!(laid_out(md, 6), vec!["Left |", "Right", "a | 1"]);
    }

    #[test]
    fn heights_count_every_row_drawn() {
        let model = render("# T\n\npara\n\n---\n", Flavor::Gfm);
        let mut cache = RenderCache::default();
        assert!(cache.update(&model, 1, 20, &Theme::default(), 0));
        // Heading + its rule; blank + para; blank + rule.
        assert_eq!(cache.heights(), vec![2, 2, 2]);
        assert!(!cache.update(&model, 1, 20, &Theme::default(), 0), "cached");
        assert!(
            cache.update(&model, 1, 30, &Theme::default(), 0),
            "a new width lays out again"
        );
    }

    /// The phase gate's terminal half: every GFM spec example lays out and
    /// draws at narrow to wide widths without panicking, and its heights add
    /// up to the rows drawn.
    #[test]
    fn every_gfm_spec_example_lays_out_and_draws() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let spec = include_str!("../../stede-markdown/tests/spec/gfm-spec.txt");
        let fence = "`".repeat(32);
        let mut examples = Vec::new();
        let mut lines = spec.lines();
        while let Some(line) = lines.next() {
            if !(line.starts_with(&fence)
                && line[fence.len()..].trim_start().starts_with("example"))
            {
                continue;
            }
            let mut md = String::new();
            for line in lines.by_ref() {
                if line == "." {
                    break;
                }
                md.push_str(&line.replace('→', "\t"));
                md.push('\n');
            }
            for line in lines.by_ref() {
                if line == fence {
                    break;
                }
            }
            examples.push(md);
        }
        assert!(examples.len() > 600, "found {}", examples.len());
        let theme = Theme::default();
        for (i, md) in examples.iter().enumerate() {
            let model = render(md, Flavor::Gfm);
            for width in [1, 10, 40, 100] {
                let blocks = layout(&model, width, &theme);
                assert_eq!(blocks.len(), model.blocks.len(), "example {}", i + 1);
                let mut terminal = Terminal::new(TestBackend::new(width as u16, 12)).unwrap();
                terminal
                    .draw(|f| {
                        let lines: Vec<Line> = blocks.iter().flatten().cloned().collect();
                        f.render_widget(ratatui::widgets::Paragraph::new(lines), f.area());
                    })
                    .unwrap();
            }
        }
    }

    #[test]
    fn column_widths_shrink_to_fit() {
        assert_eq!(fit_columns(&[3, 4], 10), vec![3, 4]);
        assert_eq!(fit_columns(&[10, 2], 8), vec![6, 2]);
        assert!(fit_columns(&[50, 50, 50], 9).iter().sum::<usize>() <= 9);
    }
}
