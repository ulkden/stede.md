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
use stede_core::modal::{self, Modal, ModalBody};
use stede_core::view::{PaneView, STATUS_ROWS, View};

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

/// Draws one frame. The view must have been built for this frame's size
/// (`App::resize` before `App::view`).
pub fn draw(frame: &mut Frame, view: &View, theme: &Theme) {
    let [panes_area, status_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(STATUS_ROWS)]).areas(frame.area());
    for pane in &view.panes {
        draw_pane(frame, panes_area, pane, theme);
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

fn draw_pane(frame: &mut Frame, area: Rect, pane: &PaneView, theme: &Theme) {
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
    let block = Block::bordered()
        .border_style(border)
        .title(Line::styled(format!(" {} ", pane.title), title_style));
    frame.render_widget(block, rect);
}

fn draw_status(frame: &mut Frame, area: Rect, view: &View, theme: &Theme) {
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
    if let Some(message) = &view.status.message {
        spans.push(Span::raw("  "));
        spans.push(Span::raw(message.clone()));
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
        app.resize(width, height);
        terminal
            .draw(|f| draw(f, &app.view(), &Theme::default()))
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
        for _ in 0..50 {
            press(&mut app, "j");
        }
        let screen = render(&mut app, 80, 16);
        assert!(screen.contains("<F1>      help.keys"), "{screen}");
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
