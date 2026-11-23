// SPDX-License-Identifier: GPL-3.0-or-later
//! Draws the core's view model with ratatui.
//!
//! Everything here reads a [`View`]; nothing reaches into core state. Widths
//! come from the core's layout and text widths from display width (ratatui
//! measures with `unicode-width`), so CJK and emoji line up.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use stede_core::App;
use stede_core::doc::LineKind;
use stede_core::layout::PaneId;
use stede_core::modal::{self, Modal, ModalBody};
use stede_core::text::wrap_rows_mapped;

pub use crate::rendered::RenderCache;
use stede_core::view::{PaneView, RawView, STATUS_ROWS, SearchView, TreeView, View};

/// The shell's colors, taken from the core's theme slots.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub accent: Color,
    pub border: Color,
    pub strip_fg: Color,
    pub strip_bg: Color,
    pub status_fg: Color,
    pub status_bg: Color,
    pub dim: Color,
    pub linenr: Color,
    pub heading: Color,
    pub code: Color,
    pub code_bg: Color,
    pub quote: Color,
    pub rule: Color,
    pub meta: Color,
    pub search_match: Color,
    pub search_current: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self::from_core(
            &stede_core::theme::Theme::default(),
            ColorSupport::Truecolor,
        )
    }
}

/// What colors the terminal can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSupport {
    /// 24-bit RGB.
    Truecolor,
    /// The xterm 256-color palette: RGB is shown as the nearest of those.
    Indexed256,
    /// The 16 ANSI colors only (the Linux console, plain `xterm`).
    Ansi16,
}

impl ColorSupport {
    /// Truecolor if `COLORTERM` says so (`truecolor` or `24bit`); 16 colors
    /// for terminals known to have only those (`TERM` of `linux`, `xterm`,
    /// `vt100` and the like, or ending in `-16color`/`-8color`); else 256,
    /// which nearly every terminal has (Terminal.app among them).
    pub fn detect(colorterm: Option<&str>, term: Option<&str>) -> Self {
        if matches!(
            colorterm.map(str::to_ascii_lowercase).as_deref(),
            Some("truecolor" | "24bit")
        ) {
            return ColorSupport::Truecolor;
        }
        let term = term.unwrap_or_default().to_ascii_lowercase();
        let low = ["linux", "xterm", "vt100", "vt220", "ansi", "cons25", "dumb"];
        if low.contains(&term.as_str()) || term.ends_with("-16color") || term.ends_with("-8color") {
            ColorSupport::Ansi16
        } else {
            ColorSupport::Indexed256
        }
    }
}

impl Theme {
    /// The shell's colors from the core theme.
    pub fn from_core(theme: &stede_core::theme::Theme, support: ColorSupport) -> Self {
        let get = |slot| to_color(theme.get(slot), support);
        Self {
            accent: get("ui.accent"),
            border: get("ui.border"),
            strip_fg: get("ui.strip.fg"),
            strip_bg: get("ui.strip.bg"),
            status_fg: get("ui.status.fg"),
            status_bg: get("ui.status.bg"),
            dim: get("ui.dim"),
            linenr: get("ui.linenr"),
            heading: get("md.heading"),
            code: get("md.code"),
            code_bg: get("md.code.bg"),
            quote: get("md.quote"),
            rule: get("md.rule"),
            meta: get("md.meta"),
            search_match: get("search.match"),
            search_current: get("search.current"),
        }
    }
}

/// A theme color as ratatui draws it on this terminal.
fn to_color(color: stede_core::theme::Color, support: ColorSupport) -> Color {
    use stede_core::theme::Color as C;
    match color {
        C::Default => Color::Reset,
        C::Ansi(i) => Color::Indexed(i),
        C::Rgb(r, g, b) => match support {
            ColorSupport::Truecolor => Color::Rgb(r, g, b),
            ColorSupport::Indexed256 => Color::Indexed(nearest_256(r, g, b)),
            ColorSupport::Ansi16 => Color::Indexed(nearest_16(r, g, b)),
        },
    }
}

/// The nearest of the 16 ANSI colors, by xterm's default values.
fn nearest_16(r: u8, g: u8, b: u8) -> u8 {
    const ANSI: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
    (0..16u8)
        .min_by_key(|&i| {
            let (x, y, z) = ANSI[usize::from(i)];
            d(x, r) + d(y, g) + d(z, b)
        })
        .expect("sixteen colors")
}

/// The nearest xterm-256 color: the 6x6x6 cube (16-231) or the gray ramp
/// (232-255), whichever is closer. The first 16 are left out: terminals
/// remap them, so they aren't a fixed target.
fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let level = |v: u8| -> usize {
        (0..6)
            .min_by_key(|&i| (i32::from(STEPS[i]) - i32::from(v)).abs())
            .expect("six steps")
    };
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = (16 + 36 * ri + 6 * gi + bi) as u8;
    let dist = |(x, y, z): (u8, u8, u8)| {
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        d(x, r) + d(y, g) + d(z, b)
    };
    let cube_rgb = (STEPS[ri], STEPS[gi], STEPS[bi]);
    let avg = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    // Grays 232-255 are 8, 18, ..., 238.
    let gray_i = (avg.saturating_sub(3) / 10).min(23) as u8;
    let gray_v = 8 + 10 * gray_i;
    if dist((gray_v, gray_v, gray_v)) < dist(cube_rgb) {
        232 + gray_i
    } else {
        cube
    }
}

/// Before drawing: lays the rendered pane out (if the document, its width or
/// the theme changed) and reports the block heights to the core, which
/// scrolls and syncs by them. Call after `App::resize`.
pub fn prepare(app: &mut App, cache: &mut RenderCache, theme: &Theme, theme_epoch: u64) {
    let (generation, width, updated) = {
        let view = app.view();
        let width = view
            .panes
            .iter()
            .find(|p| p.id == PaneId::Rendered && !p.rect.folded)
            .map_or(0, |p| usize::from(p.rect.width.saturating_sub(2)));
        let Some(model) = view.rendered.model else {
            return;
        };
        if width == 0 {
            return;
        }
        let generation = view.rendered.generation;
        let updated = cache.update(model, generation, width, theme, theme_epoch);
        (generation, width, updated)
    };
    if updated || !app.rendered_layout_is_current(generation, width) {
        app.set_rendered_layout(generation, width, cache.heights());
    }
}

/// Draws one frame. The view must have been built for this frame's size
/// (`App::resize` before `App::view`), and `prepare` run for the cache.
pub fn draw(frame: &mut Frame, view: &View, theme: &Theme, cache: &RenderCache) {
    let [panes_area, status_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(STATUS_ROWS)]).areas(frame.area());
    for pane in &view.panes {
        draw_pane(frame, panes_area, pane, view, theme, cache);
    }
    draw_status(frame, status_area, view, theme);
    let footer = view
        .close_key
        .as_deref()
        .map(|key| format!(" {key} to close "));
    for modal in &view.modals {
        draw_modal(frame, panes_area, modal, footer.as_deref(), theme);
    }
}

fn pane_rect(area: Rect, pane: &PaneView) -> Rect {
    let x = area.x.saturating_add(pane.rect.x);
    let width = pane.rect.width.min(area.right().saturating_sub(x));
    Rect::new(x, area.y, width, area.height)
}

fn draw_pane(
    frame: &mut Frame,
    area: Rect,
    pane: &PaneView,
    view: &View,
    theme: &Theme,
    cache: &RenderCache,
) {
    let rect = pane_rect(area, pane);
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    if pane.rect.folded {
        // A strip: the pane's title read top to bottom, on a band of color.
        let style = Style::new().fg(theme.strip_fg).bg(theme.strip_bg);
        let lines: Vec<Line> = pane
            .title
            .chars()
            .take(rect.height as usize)
            .map(|c| Line::from(c.to_string()))
            .collect();
        frame.render_widget(Paragraph::new(lines).style(style), rect);
        return;
    }
    let border = if pane.focused {
        Style::new().fg(theme.accent)
    } else {
        Style::new().fg(theme.border)
    };
    let title_style = if pane.focused {
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    // The tree's title names its folder, the raw pane its file.
    let title = match (pane.id, &view.tree.root, &view.raw.title) {
        (PaneId::Tree, Some(root), _) => format!(" {}: {root} ", pane.title),
        (PaneId::Raw, _, Some(file)) => format!(" {}: {file} ", pane.title),
        _ => format!(" {} ", pane.title),
    };
    let block = Block::bordered()
        .border_style(border)
        .title(Line::styled(title, title_style));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    match pane.id {
        PaneId::Tree => draw_tree(frame, inner, &view.tree, pane.focused, theme),
        PaneId::Raw => draw_raw(frame, inner, &view.raw, view.search, theme),
        PaneId::Rendered => draw_rendered(frame, inner, view, cache, theme),
    }
}

/// Context kept around the drawn bytes when matching, so `^`, `\b` and
/// matches that start just off screen still read their neighbors.
const MATCH_CONTEXT: usize = 1024;

/// The byte largest at or below `i` that starts a character.
fn char_floor(text: &str, mut i: usize) -> usize {
    i = i.min(text.len());
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Matches of `regex` in `line` touching bytes `from..to` (what's drawn),
/// sorted. Only a window around them is searched, so a huge line costs
/// what's on screen, not the whole line.
fn drawn_matches(regex: &regex::Regex, line: &str, from: usize, to: usize) -> Vec<(usize, usize)> {
    let start = char_floor(line, from.saturating_sub(MATCH_CONTEXT));
    let end = char_floor(line, to.saturating_add(MATCH_CONTEXT));
    // `find_at` keeps what's before `start` as context for `^` and `\b`.
    let hay = &line[..end];
    let mut out = Vec::new();
    let mut at = start;
    while at <= hay.len() {
        let Some(m) = regex.find_at(hay, at) else {
            break;
        };
        if m.start() >= to {
            break;
        }
        if m.start() < m.end() && m.end() > from {
            out.push((m.start(), m.end()));
        }
        at = if m.end() > m.start() {
            m.end()
        } else {
            // Step past an empty match by a whole character.
            hay[m.end()..]
                .chars()
                .next()
                .map_or(hay.len() + 1, |c| m.end() + c.len_utf8())
        };
    }
    out
}

/// A raw row split into runs, lit where a character's source byte falls in
/// a match. Both are in order, so one cursor walks the matches.
fn lit_by_offset(
    row: &str,
    offsets: &[usize],
    ranges: &[(usize, usize)],
    style: Style,
    color: Color,
) -> Vec<Span<'static>> {
    if ranges.is_empty() {
        return vec![Span::styled(row.to_owned(), style)];
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_lit = false;
    let mut next = 0;
    for (c, &at) in row.chars().zip(offsets) {
        while next < ranges.len() && ranges[next].1 <= at {
            next += 1;
        }
        let lit = ranges.get(next).is_some_and(|&(a, b)| at >= a && at < b);
        if lit != run_lit && !run.is_empty() {
            let s = if run_lit { style.bg(color) } else { style };
            spans.push(Span::styled(std::mem::take(&mut run), s));
        }
        run_lit = lit;
        run.push(c);
    }
    if !run.is_empty() {
        let s = if run_lit { style.bg(color) } else { style };
        spans.push(Span::styled(run, s));
    }
    spans
}

/// A row with every search match's background lit: `current` uses the
/// brighter slot. Matches are found in the row as drawn, so they follow
/// wrapping and tab expansion.
fn highlight(row: Line<'static>, regex: &regex::Regex, color: Color) -> Line<'static> {
    let text: String = row.spans.iter().map(|s| s.content.as_ref()).collect();
    let ranges: Vec<(usize, usize)> = regex
        .find_iter(&text)
        .filter(|m| m.start() < m.end())
        .map(|m| (m.start(), m.end()))
        .collect();
    if ranges.is_empty() {
        return row;
    }
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut offset = 0;
    for span in row.spans {
        let content = span.content.to_string();
        let (start, end) = (offset, offset + content.len());
        offset = end;
        // Cut points inside this span: match edges that fall within it.
        let mut cuts: Vec<usize> = ranges
            .iter()
            .flat_map(|&(a, b)| [a, b])
            .filter(|&c| c > start && c < end)
            .map(|c| c - start)
            .collect();
        cuts.sort_unstable();
        cuts.dedup();
        let mut last = 0;
        for cut in cuts.into_iter().chain(std::iter::once(content.len())) {
            if cut <= last || !content.is_char_boundary(cut) {
                continue;
            }
            let piece = &content[last..cut];
            let at = start + last;
            let lit = ranges.iter().any(|&(a, b)| at >= a && at < b);
            let style = if lit {
                span.style.bg(color)
            } else {
                span.style
            };
            out.push(Span::styled(piece.to_owned(), style));
            last = cut;
        }
    }
    Line::from(out)
}

/// The rendered pane: the laid-out blocks from the core's position.
fn draw_rendered(frame: &mut Frame, area: Rect, view: &View, cache: &RenderCache, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let ready = view.rendered.model.is_some()
        && cache.generation == view.rendered.generation
        && cache.width == usize::from(area.width);
    if !ready {
        // Before a file opens (or while it loads), say what the raw pane says.
        let message = view.raw.message.clone().unwrap_or_default();
        frame.render_widget(
            Paragraph::new(Line::styled(message, Style::new().fg(theme.dim))),
            area,
        );
        return;
    }
    let (block, row) = view.rendered.position;
    let height = usize::from(area.height);
    // The block holding the current match's line is lit brighter.
    let current_block = view.search.and_then(|s| {
        let line = s.current_line?;
        view.rendered
            .model?
            .blocks
            .iter()
            .position(|b| b.source.contains(line))
    });
    let lines: Vec<Line> = cache
        .blocks
        .iter()
        .enumerate()
        .skip(block)
        .flat_map(|(i, rows)| rows.iter().map(move |r| (i, r)))
        .skip(row)
        .take(height)
        .map(|(i, r)| match view.search {
            Some(s) => {
                let color = if current_block == Some(i) {
                    theme.search_current
                } else {
                    theme.search_match
                };
                highlight(r.clone(), s.regex, color)
            }
            None => r.clone(),
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// The raw pane: the file's lines from the top of the view, wrapped to the
/// pane, with an optional number column; each line colored by its block.
fn draw_raw(
    frame: &mut Frame,
    area: Rect,
    raw: &RawView,
    search: Option<SearchView>,
    theme: &Theme,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if let Some(message) = &raw.message {
        frame.render_widget(
            Paragraph::new(Line::styled(message.as_str(), Style::new().fg(theme.dim))),
            area,
        );
        return;
    }
    // The core gives the columns, so rows break where its scrolling counts
    // them; a pane drawn narrower than that (it can't be, but) just clips.
    let gutter = raw.gutter;
    let text_width = raw.text_width.max(1);
    let height = usize::from(area.height);
    let mut lines: Vec<Line> = Vec::with_capacity(height);
    for (i, (number, text, kind)) in raw.lines.iter().enumerate() {
        if lines.len() == height {
            break;
        }
        let style = line_style(*kind, theme);
        // The first line may be scrolled into; only the rows that fit are built.
        let skip = if i == 0 { raw.first_row } else { 0 };
        let color = match search {
            Some(s) if s.current_line == Some(*number) => theme.search_current,
            _ => theme.search_match,
        };
        let rows = wrap_rows_mapped(text, text_width, skip, height - lines.len());
        let ranges = match (search, rows.first(), rows.last()) {
            (Some(s), Some(first), Some(last)) => {
                let from = first.1.first().copied().unwrap_or(0);
                let to = last.1.last().map_or(from, |&b| b + 1);
                drawn_matches(s.regex, text, from, to)
            }
            _ => Vec::new(),
        };
        for (r, (row, offsets)) in rows.into_iter().enumerate() {
            let mut spans = Vec::with_capacity(2);
            if gutter > 0 {
                let label = if r + skip == 0 {
                    number.to_string()
                } else {
                    String::new()
                };
                spans.push(Span::styled(
                    format!("{label:>w$} ", w = gutter - 1),
                    Style::new().fg(theme.linenr),
                ));
            }
            spans.extend(lit_by_offset(&row, &offsets, &ranges, style, color));
            lines.push(Line::from(spans));
        }
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn line_style(kind: LineKind, theme: &Theme) -> Style {
    match kind {
        LineKind::Plain => Style::new(),
        LineKind::Heading(_) => Style::new().fg(theme.heading).add_modifier(Modifier::BOLD),
        LineKind::Code => Style::new().fg(theme.code),
        LineKind::Quote => Style::new().fg(theme.quote),
        LineKind::Rule | LineKind::Table => Style::new().fg(theme.rule),
        LineKind::Meta => Style::new().fg(theme.meta),
    }
}

/// The file tree: folders marked open (▾) or closed (▸), children indented,
/// the selection highlighted (reversed while the pane has focus).
fn draw_tree(frame: &mut Frame, area: Rect, tree: &TreeView, focused: bool, theme: &Theme) {
    // No folder at all (as in tests) draws nothing; an empty one says so.
    if area.width == 0 || area.height == 0 || tree.root.is_none() {
        return;
    }
    let dim = Style::new().fg(theme.dim);
    if let Some(error) = &tree.error {
        frame.render_widget(
            Paragraph::new(Line::styled(format!("can't list: {error}"), dim)),
            area,
        );
        return;
    }
    if tree.loading {
        frame.render_widget(Paragraph::new(Line::styled("Loading…", dim)), area);
        return;
    }
    if tree.rows.is_empty() {
        frame.render_widget(Paragraph::new(Line::styled("(empty)", dim)), area);
        return;
    }
    let height = usize::from(area.height);
    let lines: Vec<Line> = tree
        .rows
        .iter()
        .enumerate()
        .skip(tree.scroll)
        .take(height)
        .map(|(i, row)| {
            let marker = match (row.is_dir, row.expanded) {
                (true, true) => "▾ ",
                (true, false) => "▸ ",
                (false, _) => "  ",
            };
            let mut text = format!("{}{marker}{}", "  ".repeat(row.depth), row.name);
            if row.is_dir {
                text.push('/');
            }
            if row.loading {
                text.push_str(" …");
            }
            if let Some(e) = &row.error {
                text.push_str(&format!(" (can't list: {e})"));
            }
            let mut style = if row.is_dir {
                Style::new().fg(theme.accent)
            } else {
                Style::new()
            };
            if tree.selected == Some(i) {
                style = if focused {
                    style.add_modifier(Modifier::REVERSED)
                } else {
                    style.add_modifier(Modifier::BOLD)
                };
            }
            Line::styled(text, style)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_status(frame: &mut Frame, area: Rect, view: &View, theme: &Theme) {
    // Typing a search: the pattern takes the line, with a cursor after it.
    if let Some(prompt) = &view.status.prompt {
        let mut spans = vec![
            Span::raw(prompt.clone()),
            Span::styled(" ", Style::new().add_modifier(Modifier::REVERSED)),
        ];
        // A message from typing (the pattern's length limit) shows after it.
        if let Some(message) = &view.status.message {
            spans.push(Span::styled(
                format!("  {message}"),
                Style::new().fg(theme.dim),
            ));
        }
        let line = Line::from(spans);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    let mode = Span::styled(
        format!(" {} ", view.status.mode.to_uppercase()),
        Style::new()
            .fg(theme.status_fg)
            .bg(theme.status_bg)
            .add_modifier(Modifier::BOLD),
    );
    let mut spans = vec![mode];
    if !view.status.pending.is_empty() {
        spans.push(Span::raw(format!(" {}", view.status.pending)));
    }
    if let Some(s) = view.search
        && let Some((at, of)) = s.position
    {
        let more = if s.capped { "+" } else { "" };
        spans.push(Span::styled(
            format!(" [{at}/{of}{more}]"),
            Style::new().fg(theme.accent),
        ));
    }
    // The message first: it's what needs reading now, and the file name
    // after it can be cut at the edge without losing much.
    if let Some(message) = &view.status.message {
        spans.push(Span::raw("  "));
        spans.push(Span::raw(message.clone()));
    }
    if let Some(file) = &view.status.file {
        spans.push(Span::styled(
            format!("  {file}"),
            Style::new().fg(theme.dim),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The lines a modal body draws, in the shape `Modal::line_count` expects:
/// for a table, the header line, then each section's title and its rows.
fn modal_lines<'a>(modal: &'a Modal, theme: &Theme) -> Vec<Line<'a>> {
    match &modal.body {
        ModalBody::Text(lines) => lines.iter().map(|l| Line::raw(l.as_str())).collect(),
        ModalBody::Table { headers, sections } => {
            let columns = headers.len();
            let mut widths: Vec<usize> = headers
                .iter()
                .map(|h| Line::raw(h.as_str()).width())
                .collect();
            for row in sections.iter().flat_map(|s| &s.rows) {
                for (i, cell) in row.cells.iter().take(columns).enumerate() {
                    widths[i] = widths[i].max(Line::raw(cell.as_str()).width());
                }
            }
            let cell_styles = [
                Style::new().fg(theme.accent),
                Style::new().fg(theme.dim),
                Style::new(),
            ];
            let row_line = |cells: &'a [String], styles: &[Style]| {
                let mut spans = Vec::with_capacity(cells.len() * 2);
                for (i, cell) in cells.iter().take(columns).enumerate() {
                    let style = styles.get(i).copied().unwrap_or_default();
                    spans.push(Span::styled(cell.as_str(), style));
                    if i + 1 < columns {
                        let pad = widths[i].saturating_sub(Line::raw(cell.as_str()).width()) + 2;
                        spans.push(Span::raw(" ".repeat(pad)));
                    }
                }
                Line::from(spans)
            };
            let bold = Style::new().add_modifier(Modifier::BOLD);
            let mut lines = vec![row_line(headers, &[bold; 3])];
            for section in sections {
                lines.push(Line::styled(
                    section.title.as_str(),
                    Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                ));
                for row in &section.rows {
                    lines.push(row_line(&row.cells, &cell_styles));
                }
            }
            lines
        }
    }
}

fn draw_modal(frame: &mut Frame, area: Rect, modal: &Modal, footer: Option<&str>, theme: &Theme) {
    let lines = modal_lines(modal, theme);
    let content_width = lines.iter().map(Line::width).max().unwrap_or(0);
    let title = format!(" {} ", modal.title);
    let footer = footer.unwrap_or("");
    let want_width = content_width
        .max(Line::raw(title.as_str()).width())
        .max(Line::raw(footer).width())
        .saturating_add(4); // borders and a cell of padding each side
    // The core sizes modals the same way, so its scrolling matches this.
    let (max_width, max_height) = modal::max_size(area.width, area.height);
    let width = want_width.min(usize::from(max_width)).max(1) as u16;
    let height = lines
        .len()
        .saturating_add(usize::from(modal::BORDER_ROWS))
        .min(usize::from(max_height))
        .max(1) as u16;
    let rect = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width.min(area.width),
        height.min(area.height),
    );
    if rect.width < 3 || rect.height < 3 {
        return;
    }
    let block = Block::bordered()
        .border_style(Style::new().fg(theme.accent))
        .title(Line::styled(
            title,
            Style::new().add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::styled(footer, Style::new().fg(theme.dim)).right_aligned());
    let inner = block.inner(rect).inner(ratatui::layout::Margin::new(1, 0));
    let visible = usize::from(inner.height);
    let scroll = modal.scroll.min(lines.len().saturating_sub(visible));
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    frame.render_widget(
        Paragraph::new(lines).scroll((scroll.min(usize::from(u16::MAX)) as u16, 0)),
        inner,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use stede_core::key::parse_keys;
    use stede_core::layout::LayoutOptions;
    use stede_core::{App, AppOptions};

    fn draw_only(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let theme = Theme::default();
        let mut cache = RenderCache::default();
        app.resize(width, height);
        prepare(app, &mut cache, &theme, 0);
        terminal
            .draw(|f| draw(f, &app.view(), &theme, &cache))
            .unwrap();
        terminal
    }

    fn render(app: &mut App, width: u16, height: u16) -> String {
        draw_only(app, width, height).backend().to_string()
    }

    fn press(app: &mut App, keys: &str) {
        for key in parse_keys(keys, Some(app.keymap().leader())).unwrap() {
            app.handle_key(key);
        }
    }

    #[test]
    fn default_layout() {
        let mut app = App::new(AppOptions::default());
        insta::assert_snapshot!(render(&mut app, 60, 8));
    }

    #[test]
    fn single_file_starts_with_tree_folded() {
        let mut app = App::new(AppOptions {
            layout: LayoutOptions {
                tree_folded: true,
                ..LayoutOptions::default()
            },
            ..AppOptions::default()
        });
        insta::assert_snapshot!(render(&mut app, 60, 8));
    }

    #[test]
    fn focus_one_folds_the_rest_to_strips() {
        let mut app = App::new(AppOptions::default());
        press(&mut app, "<C-l><leader>z");
        insta::assert_snapshot!(render(&mut app, 40, 10));
    }

    #[test]
    fn pending_keys_and_messages_show_in_the_status_line() {
        let mut app = App::new(AppOptions::default());
        press(&mut app, "3<C-w>");
        insta::assert_snapshot!("pending", render(&mut app, 40, 4));
        press(&mut app, "x");
        insta::assert_snapshot!("unbound", render(&mut app, 40, 4));
    }

    #[test]
    fn cheat_sheet() {
        let mut app = App::new(AppOptions::default());
        press(&mut app, "<F1>");
        insta::assert_snapshot!(render(&mut app, 100, 40));
    }

    #[test]
    fn cheat_sheet_scrolls_in_a_small_terminal() {
        let mut app = App::new(AppOptions::default());
        app.resize(80, 16);
        press(&mut app, "<F1>jjj");
        insta::assert_snapshot!(render(&mut app, 80, 16));
    }

    #[test]
    fn cheat_sheet_scrolled_to_the_end_shows_its_last_line() {
        let mut app = App::new(AppOptions::default());
        app.resize(80, 16);
        press(&mut app, "<F1>");
        // Enough to reach the end, however long the sheet grows.
        for _ in 0..500 {
            press(&mut app, "j");
        }
        let screen = render(&mut app, 80, 16);
        // The last row (the modal binding for <F1>) is on screen.
        assert!(
            screen
                .lines()
                .any(|l| l.contains("<F1>") && l.contains("help.keys")),
            "{screen}"
        );
        press(&mut app, "k");
        assert_ne!(render(&mut app, 80, 16), screen, "one k moves the view");
    }

    #[test]
    fn colors_downgrade_without_truecolor() {
        assert_eq!(
            ColorSupport::detect(Some("truecolor"), Some("xterm-kitty")),
            ColorSupport::Truecolor
        );
        assert_eq!(
            ColorSupport::detect(Some("24BIT"), None),
            ColorSupport::Truecolor
        );
        assert_eq!(
            ColorSupport::detect(None, Some("xterm-256color")),
            ColorSupport::Indexed256
        );
        assert_eq!(
            ColorSupport::detect(None, Some("linux")),
            ColorSupport::Ansi16
        );
        assert_eq!(
            ColorSupport::detect(None, Some("rxvt-16color")),
            ColorSupport::Ansi16
        );
        assert_eq!(nearest_16(250, 10, 10), 9);
        assert_eq!(nearest_16(10, 10, 10), 0);
        let t = Theme::from_core(&stede_core::theme::Theme::default(), ColorSupport::Ansi16);
        assert!(matches!(t.accent, Color::Indexed(i) if i < 16));
        assert_eq!(nearest_256(0, 0, 0), 16);
        assert_eq!(nearest_256(255, 255, 255), 231);
        assert_eq!(nearest_256(255, 0, 0), 196);
        assert_eq!(nearest_256(128, 128, 128), 244);
        let core = stede_core::theme::Theme::default();
        let t = Theme::from_core(&core, ColorSupport::Indexed256);
        assert!(matches!(t.accent, Color::Indexed(_)));
        let t = Theme::from_core(&core, ColorSupport::Truecolor);
        assert!(matches!(t.accent, Color::Rgb(..)));
    }

    #[test]
    fn file_tree_pane() {
        use stede_core::io::{Entry, ServiceEvent};
        let entry = |p: &str, dir: bool| Entry {
            name: std::path::Path::new(p)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            path: p.into(),
            is_dir: dir,
        };
        let mut app = App::new(AppOptions {
            root: Some("/notes".into()),
            ..AppOptions::default()
        });
        insta::assert_snapshot!("tree_loading", render(&mut app, 70, 8));
        app.handle_event(ServiceEvent::Listed {
            dir: "/notes".into(),
            result: Ok(vec![
                entry("/notes/drafts", true),
                entry("/notes/日本語.md", false),
                entry("/notes/todo.md", false),
            ]),
        });
        press(&mut app, "2<C-l>l");
        app.handle_event(ServiceEvent::Listed {
            dir: "/notes/drafts".into(),
            result: Ok(vec![entry("/notes/drafts/idea.md", false)]),
        });
        insta::assert_snapshot!("tree_listed", render(&mut app, 70, 8));
    }

    #[test]
    fn raw_pane_with_numbers_wrapping_and_kinds() {
        use stede_core::io::{Document, ServiceEvent};
        let mut app = App::new(AppOptions {
            file: Some("/n/a.md".into()),
            ..AppOptions::default()
        });
        let text =
            "# Title\n\nA long paragraph line that wraps in a narrow pane.\n\n```\ncode\n```\n";
        app.handle_event(ServiceEvent::Opened {
            path: "/n/a.md".into(),
            result: Ok(Document {
                path: "/n/a.md".into(),
                text: text.into(),
                model: stede_markdown::render(text, stede_markdown::Flavor::Gfm),
                markdown: true,
            }),
        });
        insta::assert_snapshot!(render(&mut app, 70, 12));
    }

    #[test]
    fn a_line_taller_than_the_pane_scrolls_by_rows() {
        use stede_core::io::{Document, ServiceEvent};
        let mut app = App::new(AppOptions {
            file: Some("/n/a.md".into()),
            ..AppOptions::default()
        });
        let text = format!(
            "{}\nend\n",
            (0..40)
                .map(|i| format!("w{i:02}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        app.handle_event(ServiceEvent::Opened {
            path: "/n/a.md".into(),
            result: Ok(Document {
                path: "/n/a.md".into(),
                model: stede_markdown::render(&text, stede_markdown::Flavor::Gfm),
                text,
                markdown: true,
            }),
        });
        app.resize(40, 7);
        press(&mut app, "3j");
        insta::assert_snapshot!(render(&mut app, 40, 7));
    }

    fn open_doc(app: &mut App, text: &str) {
        use stede_core::io::{Document, ServiceEvent};
        app.handle_event(ServiceEvent::Opened {
            path: "/n/a.md".into(),
            result: Ok(Document {
                path: "/n/a.md".into(),
                text: text.into(),
                model: stede_markdown::render(text, stede_markdown::Flavor::Gfm),
                markdown: true,
            }),
        });
    }

    const SAMPLE: &str = "# Reader\n\nSome **bold** and `code`, with a [link](https://x.y).\n\n- [x] done\n- [ ] todo\n\n> [!TIP]\n> Notes here.\n\n| A | B |\n|---|--:|\n| one | 2 |\n\n```rust\nfn main() {}\n```\n\n---\n";

    #[test]
    fn rendered_pane_beside_the_raw_pane() {
        let mut app = App::new(AppOptions {
            file: Some("/n/a.md".into()),
            ..AppOptions::default()
        });
        open_doc(&mut app, SAMPLE);
        insta::assert_snapshot!(render(&mut app, 100, 30));
    }

    #[test]
    fn scrolling_one_pane_moves_the_other() {
        let mut app = App::new(AppOptions {
            file: Some("/n/a.md".into()),
            ..AppOptions::default()
        });
        let text: String = (1..=60).map(|i| format!("Paragraph {i}.\n\n")).collect();
        open_doc(&mut app, &text);
        // Draw once so the layout reaches the core.
        render(&mut app, 80, 12);
        press(&mut app, "20j");
        render(&mut app, 80, 12);
        assert_eq!(app.view().raw.lines[0].0, 21);
        assert_eq!(
            app.view().rendered.position.0,
            10,
            "line 21 is paragraph 11's"
        );
        // Now drive from the rendered pane: the raw pane follows.
        press(&mut app, "<C-l>");
        press(&mut app, "4j");
        render(&mut app, 80, 12);
        let (block, _) = app.view().rendered.position;
        assert_eq!(block, 12);
        assert_eq!(
            app.view().raw.lines[0].0,
            25,
            "paragraph 13 starts on line 25"
        );
        // Enter jumps the raw pane to the top block and focuses it.
        press(&mut app, "<CR>");
        assert_eq!(app.layout().focus(), stede_core::layout::PaneId::Raw);
    }

    #[test]
    fn g_takes_both_panes_to_their_ends() {
        let mut app = App::new(AppOptions {
            file: Some("/n/a.md".into()),
            ..AppOptions::default()
        });
        // Ends with a table: taller rendered than raw.
        let text = format!(
            "{}| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n",
            (1..=30).map(|i| format!("p{i}\n\n")).collect::<String>()
        );
        open_doc(&mut app, &text);
        render(&mut app, 80, 10);
        press(&mut app, "G");
        let screen = render(&mut app, 80, 10);
        assert!(
            screen.contains("└"),
            "the table's bottom border shows:\n{screen}"
        );
    }

    #[test]
    fn scrolling_rendered_with_raw_folded_keeps_raw_sane() {
        let mut app = App::new(AppOptions {
            file: Some("/n/a.md".into()),
            ..AppOptions::default()
        });
        let long = (0..200)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        open_doc(&mut app, &format!("{long}\n\nend\n"));
        render(&mut app, 80, 10);
        press(&mut app, "<C-l><leader>z");
        render(&mut app, 80, 10);
        press(&mut app, "10j");
        render(&mut app, 80, 10);
        press(&mut app, "<leader>z<C-h>");
        let screen = render(&mut app, 80, 10);
        // The raw pane shows text, not an empty pane scrolled past its line.
        let raw_rows: Vec<&str> = screen.lines().skip(1).take(3).collect();
        assert!(raw_rows.iter().any(|r| r.contains('w')), "{screen}");
    }

    #[test]
    fn a_huge_block_syncs_by_lines() {
        let mut app = App::new(AppOptions {
            file: Some("/n/a.md".into()),
            ..AppOptions::default()
        });
        let code = (1..=2000)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        open_doc(&mut app, &format!("```\n{code}\n```\n"));
        render(&mut app, 80, 12);
        press(&mut app, "1000j");
        render(&mut app, 80, 12);
        let (block, row) = app.view().rendered.position;
        assert_eq!(block, 0);
        assert!((990..=1010).contains(&row), "row {row}");
    }

    #[test]
    fn a_short_note_opens_at_its_top() {
        let mut app = App::new(AppOptions {
            file: Some("/n/a.md".into()),
            ..AppOptions::default()
        });
        // Fits the raw pane, but taller rendered (rules and table borders).
        open_doc(&mut app, "# One\n\n## Two\n\n| a |\n|---|\n| 1 |\n| 2 |\n");
        render(&mut app, 80, 12);
        press(&mut app, "j");
        render(&mut app, 80, 12);
        assert_eq!(app.view().rendered.position, (0, 0));
    }

    #[test]
    fn a_counted_g_from_the_rendered_pane_lands_on_the_line() {
        let mut app = App::new(AppOptions {
            file: Some("/n/a.md".into()),
            ..AppOptions::default()
        });
        let code = (1..=40)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        open_doc(&mut app, &format!("```\n{code}\n```\n"));
        render(&mut app, 80, 12);
        press(&mut app, "<C-l>6G");
        assert_eq!(app.view().raw.lines[0].0, 6);
    }

    #[test]
    fn search_prompt_and_lit_matches() {
        let mut app = App::new(AppOptions {
            file: Some("/n/a.md".into()),
            ..AppOptions::default()
        });
        open_doc(&mut app, "# Alpha\n\nalpha beta ALPHA\n\ngamma\n");
        render(&mut app, 80, 8);
        press(&mut app, "/alp");
        insta::assert_snapshot!("search_prompt", render(&mut app, 80, 8));
        press(&mut app, "<CR>");
        let mut terminal = draw_only(&mut app, 80, 8);
        let buffer = terminal.backend_mut().buffer().clone();
        // Smart case: all three "alpha"s, in both panes, have a lit background.
        let lit = (0..buffer.area.height)
            .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
            .filter(|&(x, y)| buffer[(x, y)].bg != ratatui::style::Color::Reset)
            .count();
        assert!(lit >= 5 * 5, "lit cells: {lit}");
        insta::assert_snapshot!("search_found", render(&mut app, 80, 8));
    }

    #[test]
    fn a_match_at_the_end_shows_in_the_rendered_pane() {
        let mut app = App::new(AppOptions {
            file: Some("/n/a.md".into()),
            ..AppOptions::default()
        });
        let text = format!(
            "{}| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | target |\n",
            (1..=30).map(|i| format!("p{i}\n\n")).collect::<String>()
        );
        open_doc(&mut app, &text);
        render(&mut app, 80, 10);
        press(&mut app, "/target<CR>");
        let screen = render(&mut app, 80, 10);
        let rendered: String = screen
            .lines()
            .map(|l| l.chars().skip(32).take(30).collect::<String>())
            .collect();
        assert!(rendered.contains("target"), "{screen}");
    }

    #[test]
    fn raw_matches_light_across_a_wrap() {
        let mut app = App::new(AppOptions {
            file: Some("/n/a.md".into()),
            ..AppOptions::default()
        });
        // The raw pane's text is 28 cells wide: "hello world" breaks after "hello".
        let line = format!("{}hello world", "x".repeat(23));
        open_doc(&mut app, &format!("{line}\n"));
        render(&mut app, 80, 8);
        press(&mut app, "/hello world<CR>");
        let mut terminal = draw_only(&mut app, 80, 8);
        let buffer = terminal.backend_mut().buffer().clone();
        let lit = |y: u16| {
            (1..31u16)
                .filter(|&x| buffer[(x, y)].bg != ratatui::style::Color::Reset)
                .count()
        };
        assert_eq!(lit(1) + lit(2), "hello world".len(), "lit on both rows");
    }

    #[test]
    fn drawn_matches_search_only_near_the_screen() {
        let regex = regex::Regex::new("x").unwrap();
        let line = "x".repeat(10_000_000);
        let start = std::time::Instant::now();
        let found = drawn_matches(&regex, &line, 5_000_000, 5_000_100);
        assert_eq!(found.len(), 100);
        assert!(start.elapsed() < std::time::Duration::from_millis(50));
        // Context before the window still counts for anchors and boundaries.
        let regex = regex::Regex::new(r"^a|\bb").unwrap();
        assert_eq!(drawn_matches(&regex, "ab b", 1, 4), vec![(3, 4)]);
    }

    #[test]
    fn highlight_splits_spans_at_match_edges() {
        let row = Line::from(vec![
            Span::raw("ab"),
            Span::styled("cd", Style::new().fg(Color::Red)),
        ]);
        let regex = regex::Regex::new("bc").unwrap();
        let lit = highlight(row, &regex, Color::Blue);
        let pieces: Vec<(&str, Option<Color>)> = lit
            .spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style.bg))
            .collect();
        assert_eq!(
            pieces,
            vec![
                ("a", None),
                ("b", Some(Color::Blue)),
                ("c", Some(Color::Blue)),
                ("d", None)
            ]
        );
    }

    #[test]
    fn tiny_terminals_do_not_panic() {
        let mut app = App::new(AppOptions::default());
        // Draw only: TestBackend itself can't format a zero-width screen.
        for (w, h) in [(0, 0), (1, 1), (2, 3), (5, 2), (3, 30), (0, 5), (5, 0)] {
            draw_only(&mut app, w, h);
        }
        press(&mut app, "<F1>");
        for (w, h) in [(0, 0), (1, 1), (4, 4), (10, 3)] {
            draw_only(&mut app, w, h);
        }
    }

    #[test]
    fn wide_characters_align_in_tables() {
        use stede_core::modal::{Row, Section};
        let modal = Modal::output(
            "Wide",
            ModalBody::Table {
                headers: vec!["Keys".into(), "Action".into(), "Description".into()],
                sections: vec![Section {
                    title: "Test".into(),
                    rows: vec![
                        Row {
                            cells: vec!["日本".into(), "a.b".into(), "CJK".into()],
                        },
                        Row {
                            cells: vec!["x".into(), "a.b".into(), "ASCII".into()],
                        },
                    ],
                }],
            },
        );
        let lines = modal_lines(&modal, &Theme::default());
        // Column 3 starts at the same display column on both rows.
        let col3 = |l: &Line| {
            let mut w = 0;
            for (i, span) in l.spans.iter().enumerate() {
                if i == 4 {
                    return w;
                }
                w += span.width();
            }
            w
        };
        assert_eq!(col3(&lines[2]), col3(&lines[3]));
    }
}
