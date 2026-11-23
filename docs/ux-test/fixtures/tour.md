---
title: The GFM tour
tags: [test, reader]
---

# The GFM tour

A paragraph with *emphasis*, **strong**, ~~strikethrough~~, `inline code`, a [link](https://example.com) and an autolink: https://example.com. A footnote follows.[^1]

## Lists

- One
- Two
  - Nested two
    1. Ordered inside
    2. And again
- Three

- [x] A done task
- [ ] A task to do

## Code

```rust
fn main() {
	println!("tabs are shown as spaces");
}
```

    an indented code block

## Quotes and alerts

> A plain quote
> over two lines.

> [!NOTE]
> A note.

> [!WARNING]
> - an alert
> - that opens with a list

## Table

| Left | Center | Right |
| :--- | :----: | ----: |
| a | b | c |
| a longer cell that wraps when the pane is narrow | `code` | 42 |

---

<div>Raw HTML is shown quietly.</div>

[^1]: The footnote's text.
