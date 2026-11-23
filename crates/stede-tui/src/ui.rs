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
use stede_core::modal::{Modal, ModalBody};
use stede_core::view::{PaneView, View};

/// Colors for the shell. Step 4 (config and themes) replaces this with theme
/// slots; until then it uses the 16 ANSI colors, which every terminal has.
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
        Self {
            accent: Color::Cyan,
            border: Color::DarkGray,
            strip_fg: Color::Gray,
            strip_bg: Color::DarkGray,
            status_fg: Color::Black,
            status_bg: Color::Cyan,
            dim: Color::DarkGray,
        }
    }
}

/// Share of the screen a modal may cover at most.
const MODAL_MAX_WIDTH_PCT: u16 = 80;
const MODAL_MAX_HEIGHT_PCT: u16 = 70;

/// Draws one frame. The view must have been built for `frame.area().width`.
pub fn draw(frame: &mut Frame, view: &View, theme: &Theme) {
    let [panes_area, status_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());
    for pane in &view.panes {
        draw_pane(frame, panes_area, pane, theme);
    }
    draw_status(frame, status_area, view, theme);
    for modal in &view.modals {
        draw_modal(frame, panes_area, modal, theme);
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

fn draw_modal(frame: &mut Frame, area: Rect, modal: &Modal, theme: &Theme) {
    let lines = modal_lines(modal, theme);
    let content_width = lines.iter().map(Line::width).max().unwrap_or(0);
    let title = format!(" {} ", modal.title);
    let footer = " Esc to close ";
    let want_width = content_width
        .max(Line::raw(title.as_str()).width())
        .max(footer.len())
        .saturating_add(4); // borders and a cell of padding each side
    let max_width = usize::from(area.width) * usize::from(MODAL_MAX_WIDTH_PCT) / 100;
    let max_height = usize::from(area.height) * usize::from(MODAL_MAX_HEIGHT_PCT) / 100;
    let width = want_width.min(max_width).max(1) as u16;
    let height = lines.len().saturating_add(2).min(max_height).max(1) as u16;
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

    fn draw_only(app: &App, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| draw(f, &app.view(f.area().width), &Theme::default()))
            .unwrap();
        terminal
    }

    fn render(app: &App, width: u16, height: u16) -> String {
        draw_only(app, width, height).backend().to_string()
    }

    fn press(app: &mut App, keys: &str) {
        for key in parse_keys(keys, Some(app.keymap().leader())).unwrap() {
            app.handle_key(key);
        }
    }

    #[test]
    fn default_layout() {
        let app = App::new(AppOptions::default());
        insta::assert_snapshot!(render(&app, 60, 8));
    }

    #[test]
    fn single_file_starts_with_tree_folded() {
        let app = App::new(AppOptions {
            layout: LayoutOptions {
                tree_folded: true,
                ..LayoutOptions::default()
            },
        });
        insta::assert_snapshot!(render(&app, 60, 8));
    }

    #[test]
    fn focus_one_folds_the_rest_to_strips() {
        let mut app = App::new(AppOptions::default());
        press(&mut app, "<C-l><leader>z");
        insta::assert_snapshot!(render(&app, 40, 10));
    }

    #[test]
    fn pending_keys_and_messages_show_in_the_status_line() {
        let mut app = App::new(AppOptions::default());
        press(&mut app, "3<C-w>");
        insta::assert_snapshot!("pending", render(&app, 40, 4));
        press(&mut app, "x");
        insta::assert_snapshot!("unbound", render(&app, 40, 4));
    }

    #[test]
    fn cheat_sheet() {
        let mut app = App::new(AppOptions::default());
        press(&mut app, "<F1>");
        insta::assert_snapshot!(render(&app, 100, 40));
    }

    #[test]
    fn cheat_sheet_scrolls_in_a_small_terminal() {
        let mut app = App::new(AppOptions::default());
        press(&mut app, "<F1>jjj");
        insta::assert_snapshot!(render(&app, 80, 16));
    }

    #[test]
    fn tiny_terminals_do_not_panic() {
        let mut app = App::new(AppOptions::default());
        // Draw only: TestBackend itself can't format a zero-width screen.
        for (w, h) in [(0, 0), (1, 1), (2, 3), (5, 2), (3, 30), (0, 5), (5, 0)] {
            draw_only(&app, w, h);
        }
        press(&mut app, "<F1>");
        for (w, h) in [(0, 0), (1, 1), (4, 4), (10, 3)] {
            draw_only(&app, w, h);
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
