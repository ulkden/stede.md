// SPDX-License-Identifier: GPL-3.0-or-later
//! Builds the render model from comrak's tree.

use comrak::nodes::{AlertType, AstNode, ListDelimType, ListType, NodeValue, TableAlignment};
use comrak::{Arena, Options, parse_document};

use crate::flavor::Flavor;
use crate::model::{
    AlertKind, Align, Block, BlockKind, Cell, Container, LineRange, Marker, RenderModel, Span,
    SpanStyle, Table,
};

/// Parses Markdown in a flavor and builds its render model.
pub fn render(markdown: &str, flavor: Flavor) -> RenderModel {
    let arena = Arena::new();
    let options = flavor.options();
    let root = parse_document(&arena, markdown, &options);
    let mut builder = Builder {
        blocks: Vec::new(),
        containers: Vec::new(),
        last_line: markdown.lines().count().max(1),
    };
    builder.walk(root);
    let mut blocks = builder.blocks;
    // comrak moves footnote definitions to the end of the tree; put every
    // block back where it sits in the source, so the rendered pane follows the
    // raw one and line lookups can binary-search. The sort is stable, so
    // blocks on the same line keep their order.
    blocks.sort_by_key(|b| b.source.start);
    RenderModel { blocks }
}

/// The comrak options a flavor uses, for callers that need comrak directly
/// (the spec tests compare its HTML).
pub fn comrak_options(flavor: Flavor) -> Options<'static> {
    flavor.options()
}

struct Builder {
    blocks: Vec<Block>,
    containers: Vec<Container>,
    last_line: usize,
}

impl Builder {
    fn range(&self, node: &AstNode<'_>) -> LineRange {
        let pos = node.data().sourcepos;
        // Some synthesized nodes have no position; keep them in bounds.
        let start = pos.start.line.clamp(1, self.last_line);
        let end = pos.end.line.clamp(start, self.last_line.max(start));
        LineRange { start, end }
    }

    fn push(&mut self, kind: BlockKind, source: LineRange) {
        self.blocks.push(Block {
            kind,
            source,
            containers: self.containers.clone(),
        });
        // Later blocks in the same list item or footnote are continuations.
        for c in self.containers.iter_mut() {
            match c {
                Container::ListItem { continuation, .. }
                | Container::Footnote { continuation, .. } => *continuation = true,
                _ => {}
            }
        }
    }

    /// Walks a container's children inside `container`; an empty container
    /// still gets one empty paragraph so its marker or bar shows.
    fn within<'a>(&mut self, node: &'a AstNode<'a>, container: Container) {
        self.containers.push(container);
        let before = self.blocks.len();
        for child in node.children() {
            self.walk(child);
        }
        if self.blocks.len() == before {
            let range = self.range(node);
            self.push(BlockKind::Paragraph(Vec::new()), range);
        }
        self.containers.pop();
    }

    fn walk<'a>(&mut self, node: &'a AstNode<'a>) {
        let range = self.range(node);
        let value = node.data().value.clone();
        match value {
            NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(_) => {
                self.within(node, Container::Quote);
            }
            NodeValue::Alert(alert) => self.within(
                node,
                Container::Alert {
                    kind: alert_kind(alert.alert_type),
                    title: alert.title.clone(),
                },
            ),
            NodeValue::List(list) => {
                for (i, item) in node.children().enumerate() {
                    let marker = match list.list_type {
                        ListType::Bullet => Marker::Bullet(char::from(list.bullet_char)),
                        ListType::Ordered => Marker::Ordered {
                            number: list.start.saturating_add(i),
                            delimiter: match list.delimiter {
                                ListDelimType::Period => '.',
                                ListDelimType::Paren => ')',
                            },
                        },
                    };
                    let task = match &item.data().value {
                        NodeValue::TaskItem(t) => Some(t.symbol.is_some()),
                        _ => None,
                    };
                    self.within(
                        item,
                        Container::ListItem {
                            marker,
                            task,
                            continuation: false,
                        },
                    );
                }
            }
            NodeValue::FootnoteDefinition(def) => self.within(
                node,
                Container::Footnote {
                    name: def.name.clone(),
                    continuation: false,
                },
            ),
            NodeValue::Paragraph => {
                let text = inlines(node);
                self.push(BlockKind::Paragraph(text), range);
            }
            NodeValue::Heading(h) => {
                let text = inlines(node);
                self.push(
                    BlockKind::Heading {
                        level: h.level,
                        text,
                    },
                    range,
                );
            }
            NodeValue::CodeBlock(code) => {
                let lang = code.info.split_whitespace().next().map(str::to_owned);
                let text = code.literal.strip_suffix('\n').unwrap_or(&code.literal);
                self.push(
                    BlockKind::Code {
                        lang,
                        text: text.to_owned(),
                    },
                    range,
                );
            }
            NodeValue::HtmlBlock(html) => {
                let text = html.literal.strip_suffix('\n').unwrap_or(&html.literal);
                self.push(BlockKind::Html(text.to_owned()), range);
            }
            NodeValue::ThematicBreak => self.push(BlockKind::Rule, range),
            NodeValue::FrontMatter(text) => {
                self.push(BlockKind::FrontMatter(strip_fences(&text)), range);
            }
            NodeValue::Table(table) => {
                let align = table.alignments.iter().map(|&a| align(a)).collect();
                let mut rows = node
                    .children()
                    .map(|row| row.children().map(inlines).collect());
                let header: Vec<Cell> = rows.next().unwrap_or_default();
                let rows: Vec<Vec<Cell>> = rows.collect();
                self.push(
                    BlockKind::Table(Table {
                        align,
                        header,
                        rows,
                    }),
                    range,
                );
            }
            // The document, and containers this flavor doesn't style
            // specially: their children carry the content.
            _ => {
                for child in node.children() {
                    self.walk(child);
                }
            }
        }
    }
}

fn alert_kind(t: AlertType) -> AlertKind {
    match t {
        AlertType::Note => AlertKind::Note,
        AlertType::Tip => AlertKind::Tip,
        AlertType::Important => AlertKind::Important,
        AlertType::Warning => AlertKind::Warning,
        AlertType::Caution => AlertKind::Caution,
    }
}

fn align(a: TableAlignment) -> Align {
    match a {
        TableAlignment::None => Align::None,
        TableAlignment::Left => Align::Left,
        TableAlignment::Center => Align::Center,
        TableAlignment::Right => Align::Right,
    }
}

/// Front matter as comrak gives it includes its `---` fences; keep the body.
fn strip_fences(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let body = match lines.as_slice() {
        [first, middle @ .., last]
            if first.trim() == "---" && matches!(last.trim(), "---" | "...") =>
        {
            middle
        }
        [first, rest @ ..] if first.trim() == "---" => rest,
        all => all,
    };
    body.join("\n")
}

/// The inline content of a node as styled spans, with neighbours of the same
/// style merged.
fn inlines<'a>(node: &'a AstNode<'a>) -> Vec<Span> {
    let mut out = Vec::new();
    for child in node.children() {
        inline(child, SpanStyle::default(), None, &mut out);
    }
    out
}

fn inline<'a>(node: &'a AstNode<'a>, style: SpanStyle, target: Option<&str>, out: &mut Vec<Span>) {
    let value = node.data().value.clone();
    let mut style = style;
    let mut target = target.map(str::to_owned);
    match value {
        NodeValue::Text(text) => return emit(out, &text, style, target),
        NodeValue::SoftBreak => return emit(out, " ", style, target),
        NodeValue::LineBreak => return emit(out, "\n", style, target),
        NodeValue::Code(code) => {
            style.code = true;
            return emit(out, &code.literal, style, target);
        }
        NodeValue::HtmlInline(html) | NodeValue::Raw(html) => {
            style.html = true;
            return emit(out, &html, style, target);
        }
        NodeValue::Math(math) => {
            style.code = true;
            return emit(out, &math.literal, style, target);
        }
        NodeValue::FootnoteReference(r) => {
            style.footnote_ref = true;
            return emit(out, &format!("[^{}]", r.name), style, target);
        }
        NodeValue::EscapedTag(tag) => return emit(out, tag, style, target),
        NodeValue::Emph => style.emph = true,
        NodeValue::Strong => style.strong = true,
        NodeValue::Strikethrough => style.strike = true,
        NodeValue::Underline => style.underline = true,
        NodeValue::Link(link) => {
            style.link = true;
            target = Some(link.url.clone());
        }
        NodeValue::WikiLink(link) => {
            style.link = true;
            target = Some(link.url.clone());
        }
        NodeValue::Image(link) => {
            style.image = true;
            target = Some(link.url.clone());
        }
        _ => {}
    }
    for child in node.children() {
        inline(child, style, target.as_deref(), out);
    }
}

fn emit(out: &mut Vec<Span>, text: &str, style: SpanStyle, target: Option<String>) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = out.last_mut()
        && last.style == style
        && last.target == target
    {
        last.text.push_str(text);
        return;
    }
    out.push(Span {
        text: text.to_owned(),
        style,
        target,
    });
}
