// SPDX-License-Identifier: GPL-3.0-or-later
//! Builds the render model from comrak's tree.
//!
//! The walk is iterative, with an explicit stack: Markdown nests without limit
//! (20,000 `>` is a valid block quote), and a recursive walk would overflow
//! the thread's stack, which can't be caught.

use comrak::nodes::{AlertType, AstNode, ListDelimType, ListType, NodeValue, TableAlignment};
use comrak::{Arena, Options, parse_document};

use crate::flavor::Flavor;
use crate::model::{
    AlertKind, Align, Block, BlockKind, Cell, Container, LineRange, Marker, RenderModel, Span,
    SpanStyle, Table,
};

/// The most containers a block lists. Deeper nesting still renders its
/// content; it just isn't drawn with more bars and indents than a screen holds.
pub const MAX_CONTAINERS: usize = 32;

/// Parses Markdown in a flavor and builds its render model.
pub fn render(markdown: &str, flavor: Flavor) -> RenderModel {
    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &flavor.options());
    let mut builder = Builder {
        blocks: Vec::new(),
        containers: Vec::new(),
        last_line: line_count(markdown),
    };
    builder.walk(root);
    RenderModel {
        blocks: builder.blocks,
    }
}

/// The comrak options `render` parses with, for callers that need comrak
/// directly (the spec tests).
pub fn comrak_options(flavor: Flavor) -> Options<'static> {
    flavor.options()
}

/// Lines in `text`, counted as comrak counts them: `\n`, `\r\n` and a lone
/// `\r` each end a line. At least 1, so an empty file has a first line.
pub fn line_count(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut lines = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => lines += 1,
            b'\r' => {
                lines += 1;
                if bytes.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if !text.is_empty() && !text.ends_with(['\n', '\r']) {
        lines += 1;
    }
    lines.max(1)
}

/// Work left on the walk's stack.
enum Task<'a> {
    /// Visit a node.
    Visit(&'a AstNode<'a>),
    /// Enter a container (if not too deep), then visit its children.
    Enter(&'a AstNode<'a>, Container),
    /// Leave a container: give it an empty paragraph if it produced no
    /// blocks, so its marker or bar still shows.
    Leave {
        node: &'a AstNode<'a>,
        blocks_before: usize,
        pushed: bool,
    },
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

    fn walk<'a>(&mut self, root: &'a AstNode<'a>) {
        let mut stack = vec![Task::Visit(root)];
        while let Some(task) = stack.pop() {
            match task {
                Task::Visit(node) => self.visit(node, &mut stack),
                Task::Enter(node, container) => {
                    let pushed = self.containers.len() < MAX_CONTAINERS;
                    if pushed {
                        self.containers.push(container);
                    }
                    stack.push(Task::Leave {
                        node,
                        blocks_before: self.blocks.len(),
                        pushed,
                    });
                    push_children(node, &mut stack);
                }
                Task::Leave {
                    node,
                    blocks_before,
                    pushed,
                } => {
                    if self.blocks.len() == blocks_before {
                        let range = self.range(node);
                        self.push(BlockKind::Paragraph(Vec::new()), range);
                    }
                    if pushed {
                        self.containers.pop();
                    }
                }
            }
        }
    }

    fn visit<'a>(&mut self, node: &'a AstNode<'a>, stack: &mut Vec<Task<'a>>) {
        let range = self.range(node);
        let data = node.data();
        match &data.value {
            NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(_) => {
                stack.push(Task::Enter(node, Container::Quote));
            }
            NodeValue::Alert(alert) => stack.push(Task::Enter(
                node,
                Container::Alert {
                    kind: alert_kind(alert.alert_type),
                    title: alert.title.clone(),
                },
            )),
            NodeValue::List(list) => {
                let items: Vec<_> = node.children().collect();
                for (i, item) in items.into_iter().enumerate().rev() {
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
                    stack.push(Task::Enter(
                        item,
                        Container::ListItem {
                            marker,
                            task,
                            continuation: false,
                        },
                    ));
                }
            }
            NodeValue::FootnoteDefinition(def) => stack.push(Task::Enter(
                node,
                Container::Footnote {
                    name: def.name.clone(),
                    continuation: false,
                },
            )),
            NodeValue::Paragraph => self.push(BlockKind::Paragraph(inlines(node)), range),
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
                self.push(BlockKind::FrontMatter(strip_fences(text)), range);
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
            _ => push_children(node, stack),
        }
    }
}

/// Queues a node's children so they're visited in order.
fn push_children<'a>(node: &'a AstNode<'a>, stack: &mut Vec<Task<'a>>) {
    let children: Vec<_> = node.children().collect();
    stack.extend(children.into_iter().rev().map(Task::Visit));
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
    let mut lines: Vec<&str> = text.lines().collect();
    // comrak keeps blank lines that follow the closing fence.
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
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

/// Inline context inherited from the nodes around a span.
#[derive(Clone, Default)]
struct Inherited {
    style: SpanStyle,
    link: Option<String>,
    image: Option<String>,
}

/// The inline content of a node as styled spans, with neighbours of the same
/// style merged. Iterative, like the block walk: emphasis nests without limit.
fn inlines<'a>(node: &'a AstNode<'a>) -> Vec<Span> {
    let mut out = Vec::new();
    let mut stack: Vec<(&'a AstNode<'a>, Inherited)> = node
        .children()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|c| (c, Inherited::default()))
        .collect();
    while let Some((node, mut ctx)) = stack.pop() {
        let data = node.data();
        match &data.value {
            NodeValue::Text(text) => {
                emit(&mut out, text, &ctx);
                continue;
            }
            NodeValue::SoftBreak => {
                emit(&mut out, " ", &ctx);
                continue;
            }
            NodeValue::LineBreak => {
                emit(&mut out, "\n", &ctx);
                continue;
            }
            NodeValue::Code(code) => {
                ctx.style.code = true;
                emit(&mut out, &code.literal, &ctx);
                continue;
            }
            NodeValue::HtmlInline(html) | NodeValue::Raw(html) => {
                ctx.style.html = true;
                emit(&mut out, html, &ctx);
                continue;
            }
            NodeValue::Math(math) => {
                ctx.style.code = true;
                emit(&mut out, &math.literal, &ctx);
                continue;
            }
            NodeValue::FootnoteReference(r) => {
                ctx.style.footnote_ref = true;
                emit(&mut out, &format!("[^{}]", r.name), &ctx);
                continue;
            }
            NodeValue::EscapedTag(tag) => {
                emit(&mut out, tag, &ctx);
                continue;
            }
            NodeValue::Emph => ctx.style.emph = true,
            NodeValue::Strong => ctx.style.strong = true,
            NodeValue::Strikethrough => ctx.style.strike = true,
            NodeValue::Underline => ctx.style.underline = true,
            NodeValue::Link(link) => {
                ctx.style.link = true;
                ctx.link = Some(link.url.clone());
            }
            NodeValue::WikiLink(link) => {
                ctx.style.link = true;
                ctx.link = Some(link.url.clone());
            }
            // An image inside a link (a README badge) keeps the link's target.
            NodeValue::Image(link) => {
                ctx.style.image = true;
                ctx.image = Some(link.url.clone());
            }
            _ => {}
        }
        let children: Vec<_> = node.children().collect();
        stack.extend(children.into_iter().rev().map(|c| (c, ctx.clone())));
    }
    out
}

fn emit(out: &mut Vec<Span>, text: &str, ctx: &Inherited) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = out.last_mut()
        && last.style == ctx.style
        && last.target == ctx.link
        && last.image == ctx.image
    {
        last.text.push_str(text);
        return;
    }
    out.push(Span {
        text: text.to_owned(),
        style: ctx.style,
        target: ctx.link.clone(),
        image: ctx.image.clone(),
    });
}
