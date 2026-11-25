// SPDX-License-Identifier: GPL-3.0-or-later
//! The phase 1 gate: the GFM spec examples render correctly.
//!
//! Every example in the GitHub Flavored Markdown spec (`tests/spec/`) is run
//! through the GFM flavor, and each must hold all of these:
//!
//! 1. Rendering doesn't panic.
//! 2. Block source ranges are in bounds and in order, which scroll sync relies on.
//! 3. The render model keeps every piece of text comrak parsed: nothing is
//!    dropped or duplicated on the way from comrak's tree to our blocks.
//! 4. With our GFM options, comrak's HTML for the example matches the spec's
//!    expected HTML, which proves the parser is configured as GFM.
//!
//! Known differences for (4) are listed in `HTML_EXCEPTIONS` with a reason.
//! The test fails if one disappears too, so the list stays honest.

use std::collections::BTreeMap;
use std::panic;

use comrak::nodes::{AstNode, NodeValue};
use comrak::{Arena, markdown_to_html, parse_document};
use stede_markdown::model::{BlockKind, Span};
use stede_markdown::{Flavor, comrak_options, line_count, render};

const SPEC: &str = include_str!("spec/gfm-spec.txt");

/// Examples whose comrak HTML differs from the spec's, and why. Each is about
/// HTML output or an extension we turn on beyond the spec, not about what the
/// rendered pane shows.
const HTML_EXCEPTIONS: &[(usize, &str)] = &[
    (66, FRONT_MATTER),
    (279, CHECKBOX_HTML),
    (280, CHECKBOX_HTML),
    (398, NESTED_STRONG),
    (426, NESTED_STRONG),
    (434, NESTED_STRONG),
    (435, NESTED_STRONG),
    (436, NESTED_STRONG),
    (473, NESTED_STRONG),
    (474, NESTED_STRONG),
    (475, NESTED_STRONG),
    (477, NESTED_STRONG),
    (610, AUTOLINK_IN_CORE),
    (616, AUTOLINK_IN_CORE),
    (619, AUTOLINK_IN_CORE),
    (620, AUTOLINK_IN_CORE),
    (652, NO_TAGFILTER),
];

const FRONT_MATTER: &str = "front matter is on: a document opening with `---` ... `---` is \
    front matter, as GitHub treats it, so this example's first rule and heading are front matter";
const CHECKBOX_HTML: &str = "task boxes serialize with attributes in another order and as \
    self-closing tags; the parse (checked or not) is the same";
const NESTED_STRONG: &str = "comrak nests <strong> where GFM's renderer collapses it; both are \
    bold, and the render model's style flags are identical";
const AUTOLINK_IN_CORE: &str = "a core (non-extension) example run with the autolink extension \
    on, which links bare URLs and emails as GFM does";
const NO_TAGFILTER: &str = "comrak has no tagfilter (still true in 1.2); it only escapes tags in HTML output, \
    which the terminal never produces";

struct Example {
    number: usize,
    section: String,
    markdown: String,
    html: String,
}

/// Reads the spec's examples. Each is fenced by 32 backticks with `example`
/// after the opening fence; a line holding only `.` separates the Markdown
/// from the expected HTML. `→` stands for a tab.
fn examples() -> Vec<Example> {
    let fence = "`".repeat(32);
    let mut out = Vec::new();
    let mut section = String::new();
    let mut lines = SPEC.lines();
    while let Some(line) = lines.next() {
        if let Some(title) = line.strip_prefix("## ") {
            section = title.to_owned();
            continue;
        }
        if !(line.starts_with(&fence) && line[fence.len()..].trim_start().starts_with("example")) {
            continue;
        }
        let mut markdown = String::new();
        let mut html = String::new();
        let mut in_html = false;
        for line in lines.by_ref() {
            if line == fence {
                break;
            }
            if line == "." && !in_html {
                in_html = true;
                continue;
            }
            let target = if in_html { &mut html } else { &mut markdown };
            target.push_str(&line.replace('→', "\t"));
            target.push('\n');
        }
        out.push(Example {
            number: out.len() + 1,
            section: section.clone(),
            markdown,
            html,
        });
    }
    out
}

/// All text comrak parsed, in order.
fn comrak_text(markdown: &str) -> String {
    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &comrak_options(Flavor::Gfm));
    let mut out = String::new();
    collect(root, &mut out);
    out
}

fn collect<'a>(node: &'a AstNode<'a>, out: &mut String) {
    match &node.data().value {
        NodeValue::Text(t) => out.push_str(t),
        NodeValue::Code(c) => out.push_str(&c.literal),
        NodeValue::CodeBlock(c) => out.push_str(&c.literal),
        NodeValue::HtmlInline(h) | NodeValue::Raw(h) => out.push_str(h),
        NodeValue::HtmlBlock(h) => out.push_str(&h.literal),
        NodeValue::Math(m) => out.push_str(&m.literal),
        NodeValue::FrontMatter(f) => {
            // Drop only the opening and closing fences, as the model does.
            let mut lines: Vec<&str> = f.lines().collect();
            while lines.last().is_some_and(|l| l.trim().is_empty()) {
                lines.pop();
            }
            if lines.first().is_some_and(|l| l.trim() == "---") {
                lines.remove(0);
            }
            if lines.last().is_some_and(|l| l.trim() == "---") {
                lines.pop();
            }
            out.push_str(&lines.concat());
        }
        _ => {}
    }
    for child in node.children() {
        collect(child, out);
    }
}

/// All text in the render model, in order. Footnote references are text we
/// add (`[^name]`), so they're left out.
fn model_text(markdown: &str) -> String {
    let model = render(markdown, Flavor::Gfm);
    let mut out = String::new();
    let spans = |spans: &[Span], out: &mut String| {
        for s in spans.iter().filter(|s| !s.style.footnote_ref) {
            out.push_str(&s.text);
        }
    };
    for block in &model.blocks {
        match &block.kind {
            BlockKind::Heading { text, .. } | BlockKind::Paragraph(text) => spans(text, &mut out),
            BlockKind::Code { text, .. } | BlockKind::Html(text) | BlockKind::FrontMatter(text) => {
                out.push_str(text)
            }
            BlockKind::Table(t) => {
                for cell in t.header.iter().chain(t.rows.iter().flatten()) {
                    spans(cell, &mut out);
                }
            }
            BlockKind::Rule => {}
        }
    }
    out
}

fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

fn html_for(markdown: &str) -> String {
    let mut options = comrak_options(Flavor::Gfm);
    // The spec expects raw HTML passed through, and footnotes handled the
    // standard way (the render model keeps definitions in place instead).
    options.render.r#unsafe = true;
    options.parse.leave_footnote_definitions = false;
    markdown_to_html(markdown, &options)
}

#[test]
fn spec_examples_render() {
    let examples = examples();
    assert!(
        examples.len() > 600,
        "expected the full GFM spec, found {} examples",
        examples.len()
    );
    let mut failures = Vec::new();
    for ex in &examples {
        let line_count = line_count(&ex.markdown);
        let rendered = panic::catch_unwind(|| render(&ex.markdown, Flavor::Gfm));
        let Ok(model) = rendered else {
            failures.push(format!("#{} ({}): render panicked", ex.number, ex.section));
            continue;
        };
        // Starts and ends both non-decreasing: block_at_line binary-searches ends.
        let mut previous = (0, 0);
        for block in &model.blocks {
            let r = block.source;
            if r.start < 1
                || r.end < r.start
                || r.end > line_count
                || (r.start, r.end) < previous
                || r.end < previous.1
            {
                failures.push(format!(
                    "#{} ({}): bad source range {r:?} (lines: {line_count}, previous {previous:?})",
                    ex.number, ex.section
                ));
            }
            previous = (r.start, r.end);
        }
        let (want, got) = (
            squash(&comrak_text(&ex.markdown)),
            squash(&model_text(&ex.markdown)),
        );
        if want != got {
            failures.push(format!(
                "#{} ({}): text lost or changed\n  comrak: {want:?}\n  model:  {got:?}",
                ex.number, ex.section
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn spec_html_matches_with_our_gfm_options() {
    let exceptions: BTreeMap<usize, &str> = HTML_EXCEPTIONS.iter().copied().collect();
    let mut mismatched = BTreeMap::new();
    for ex in examples() {
        let got = html_for(&ex.markdown);
        if got != ex.html {
            mismatched.insert(ex.number, (ex.section, ex.markdown, ex.html, got));
        }
    }
    let unexpected: Vec<String> = mismatched
        .iter()
        .filter(|(n, _)| !exceptions.contains_key(n))
        .map(|(n, (section, md, want, got))| {
            format!("#{n} ({section})\n--- markdown\n{md}--- want\n{want}--- got\n{got}")
        })
        .collect();
    let stale: Vec<&usize> = exceptions
        .keys()
        .filter(|n| !mismatched.contains_key(n))
        .collect();
    assert!(
        unexpected.is_empty() && stale.is_empty(),
        "{} unexpected HTML mismatches, stale exceptions {stale:?}:\n{}",
        unexpected.len(),
        unexpected.join("\n")
    );
}
