// SPDX-License-Identifier: GPL-3.0-or-later
//! The render model's shape for each kind of block, and the line lookups
//! scroll sync uses.

use std::fmt::Write;

use stede_markdown::model::{BlockKind, Container, Marker, Span};
use stede_markdown::{Flavor, RenderModel, render};

/// A readable dump: one line per block with its source lines, containers and content.
fn dump(model: &RenderModel) -> String {
    let spans = |spans: &[Span]| {
        spans
            .iter()
            .map(|s| {
                let mut flags = Vec::new();
                let st = s.style;
                for (on, name) in [
                    (st.emph, "em"),
                    (st.strong, "strong"),
                    (st.strike, "del"),
                    (st.code, "code"),
                    (st.link, "link"),
                    (st.image, "img"),
                    (st.footnote_ref, "fn"),
                    (st.html, "html"),
                ] {
                    if on {
                        flags.push(name);
                    }
                }
                let mut target = s
                    .target
                    .as_deref()
                    .map(|t| format!(" -> {t}"))
                    .unwrap_or_default();
                if let Some(src) = &s.image {
                    target.push_str(&format!(" src={src}"));
                }
                if flags.is_empty() {
                    format!("{:?}", s.text)
                } else {
                    format!("[{} {:?}{target}]", flags.join("+"), s.text)
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut out = String::new();
    for b in &model.blocks {
        let containers: Vec<String> = b
            .containers
            .iter()
            .map(|c| match c {
                Container::Quote => "quote".to_owned(),
                Container::Alert { kind, .. } => format!("alert:{kind:?}"),
                Container::ListItem {
                    marker,
                    task,
                    continuation,
                } => {
                    let m = match marker {
                        Marker::Bullet(c) => c.to_string(),
                        Marker::Ordered { number, delimiter } => format!("{number}{delimiter}"),
                    };
                    let t = match task {
                        Some(true) => " [x]",
                        Some(false) => " [ ]",
                        None => "",
                    };
                    let cont = if *continuation { " cont" } else { "" };
                    format!("item {m}{t}{cont}")
                }
                Container::Footnote { name, continuation } => {
                    format!(
                        "footnote {name}{}",
                        if *continuation { " cont" } else { "" }
                    )
                }
            })
            .collect();
        let content = match &b.kind {
            BlockKind::Heading { level, text } => format!("h{level} {}", spans(text)),
            BlockKind::Paragraph(text) => format!("p {}", spans(text)),
            BlockKind::Code { lang, text } => format!("code {lang:?} {text:?}"),
            BlockKind::Html(text) => format!("html {text:?}"),
            BlockKind::Rule => "rule".to_owned(),
            BlockKind::FrontMatter(text) => format!("front-matter {text:?}"),
            BlockKind::Table(t) => {
                let row = |cells: &[Vec<Span>]| {
                    cells
                        .iter()
                        .map(|c| spans(c))
                        .collect::<Vec<_>>()
                        .join(" | ")
                };
                let mut s = format!("table {:?}\n      header: {}", t.align, row(&t.header));
                for r in &t.rows {
                    write!(s, "\n      row:    {}", row(r)).unwrap();
                }
                s
            }
        };
        let prefix = if containers.is_empty() {
            String::new()
        } else {
            format!("[{}] ", containers.join(" > "))
        };
        writeln!(
            out,
            "{:>2}-{:<2} {prefix}{content}",
            b.source.start, b.source.end
        )
        .unwrap();
    }
    out
}

const DOC: &str = r#"---
title: Sample
tags: [a, b]
---
# Heading *one*

Some **bold**, `code`, ~~gone~~ and a [link](https://example.com "t").
Next line, then a hard break\
here. An image: ![alt text](pic.png) and www.example.com.

## Lists

- one
- two
  - [ ] nested task
  - [x] done task

  Still in two.
3. three
4) four

> Quoted
>
> > Nested quote

> [!WARNING]
> Careful now.

| Left | Mid | Right |
|:-----|:---:|------:|
| a    | *b* | `c`   |

```rust title="x"
fn main() {}
```

<div>raw</div>

---

A note.[^n]

[^n]: The footnote.

    More of it.
"#;

#[test]
fn gfm_document() {
    insta::assert_snapshot!(dump(&render(DOC, Flavor::Gfm)));
}

#[test]
fn commonmark_has_no_extensions() {
    insta::assert_snapshot!(dump(&render(
        "| a |\n|---|\n| b |\n\n- [ ] task\n\n~~del~~ https://x.y\n",
        Flavor::CommonMark
    )));
}

#[test]
fn empty_containers_still_show() {
    insta::assert_snapshot!(dump(&render("- \n\n>\n\n1.\n", Flavor::Gfm)));
}

#[test]
fn deep_nesting_does_not_overflow_the_stack() {
    // Run where a worker would: a thread with a small stack.
    let doc = ">".repeat(20_000) + " x\n";
    let emph = "*".repeat(50_000) + "a" + &"*".repeat(50_000) + "\n";
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            let m = render(&doc, Flavor::Gfm);
            assert_eq!(m.blocks.len(), 1);
            assert_eq!(m.blocks[0].containers.len(), stede_markdown::MAX_CONTAINERS);
            render(&emph, Flavor::Gfm);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn linked_images_keep_the_link() {
    insta::assert_snapshot!(dump(&render(
        "[![ci](badge.svg)](https://ci.example) ![plain](p.png)\n",
        Flavor::Gfm
    )));
}

#[test]
fn lone_cr_line_endings_keep_their_lines() {
    let model = render("a\rb\r\rc\r", Flavor::Gfm);
    let ranges: Vec<_> = model
        .blocks
        .iter()
        .map(|b| (b.source.start, b.source.end))
        .collect();
    assert_eq!(ranges, vec![(1, 2), (4, 4)]);
    assert_eq!(stede_markdown::line_count("a\rb\r\nc"), 3);
    assert_eq!(stede_markdown::line_count(""), 1);
}

#[test]
fn footnotes_unreferenced_or_nested_stay_put() {
    insta::assert_snapshot!(dump(&render(
        "[^a]: unreferenced\n\n> q\n>\n> [^x]: in a quote\n>\n> after\n\nref[^x]\n",
        Flavor::Gfm
    )));
}

#[test]
fn carriage_returns_never_reach_the_model() {
    for doc in [
        "```\r\nfoo\r\nbar\r\n```\r\n",
        "    code\r\n    more\r\n",
        "<div>\r\nx\r\n</div>\r\n",
        "```\rfoo\r```\r",
        "a <span\r\nx> b\r\n",
        "---\r\na: 1\r\n---\r\n\r\nbody\r\n",
    ] {
        let dumped = dump(&render(doc, Flavor::Gfm));
        assert!(!dumped.contains("\\r"), "{doc:?} => {dumped}");
    }
}

#[test]
fn adjacent_links_and_footnotes_stay_separate() {
    insta::assert_snapshot!(dump(&render(
        "x[^1][^2] [a](u)[b](u) <span\nx>\n\n[^1]: one\n[^2]: two\n",
        Flavor::Gfm
    )));
}

#[test]
fn flavor_names_ignore_case() {
    assert_eq!("GFM".parse::<Flavor>(), Ok(Flavor::Gfm));
    assert_eq!("CommonMark".parse::<Flavor>(), Ok(Flavor::CommonMark));
}

#[test]
fn front_matter_with_blank_lines_after_it() {
    let model = render("---\na: 1\n---\n\n\n# H\n", Flavor::Gfm);
    assert_eq!(model.blocks[0].kind, BlockKind::FrontMatter("a: 1".into()));
}

#[test]
fn line_lookups_for_scroll_sync() {
    let model = render("# A\n\npara\none\n\n- x\n- y\n", Flavor::Gfm);
    let starts: Vec<usize> = model.blocks.iter().map(|b| b.source.start).collect();
    assert_eq!(starts, vec![1, 3, 6, 7]);
    // A line inside a block finds it; a blank line finds the next block.
    assert_eq!(model.block_at_line(1), Some(0));
    assert_eq!(model.block_at_line(2), Some(1));
    assert_eq!(model.block_at_line(4), Some(1));
    assert_eq!(model.block_at_line(5), Some(2));
    assert_eq!(model.block_at_line(7), Some(3));
    // Past the end: the last block.
    assert_eq!(model.block_at_line(99), Some(3));
    assert_eq!(model.line_of_block(2), Some(6));
    assert_eq!(model.line_of_block(9), None);
    assert_eq!(render("", Flavor::Gfm).block_at_line(1), None);
}

#[test]
fn footnotes_stay_where_they_are_written() {
    // comrak moves definitions to the end of its tree; the model keeps source order.
    let model = render("[^a]: first\n\nbody[^a]\n", Flavor::Gfm);
    let starts: Vec<usize> = model.blocks.iter().map(|b| b.source.start).collect();
    assert_eq!(starts, vec![1, 3]);
}
