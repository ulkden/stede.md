# STEDE.md

A terminal-first Markdown reader and writer for macOS, Linux and WSL. Raw Markdown and a rendered view side by side, a file tree on the right, Vim keys by default, and every key, color and setting configurable. The main use is writing and reading Markdown for AI workflows; the design leaves room for narrative writing, scripts and technical docs later.

The full design is in `docs/architecture.md`. Read the section you need before working on an area. This file holds the rules that must not drift.

## Decisions (settled; do not reopen without asking)

- **Language and UI:** Rust. Terminal front end first with ratatui on crossterm. No Electron, Tauri or any web view. A native window (iced or Slint) is phase 4 and not being built now.
- **Name:** STEDE.md. Short name `stede` for the binary, crates and paths: `~/.config/stede/` (honor `XDG_CONFIG_HOME`) and a per-project `.stede/` folder.
- **License:** GPL-3.0-or-later. Every dependency must pass `cargo deny check`; see `deny.toml`.
- **Rendered pane is read-only.** Writing happens only in the raw pane.
- **Flavors, in order:** GFM (default), CommonMark, Obsidian-style, MkDocs.
- **Linting:** bring your own, through one adapter (LSP preferred, CLI as fallback). rumdl is the suggested default; never bundle it.
- **Plugins:** Steel (a Scheme dialect), phase 3. The first plugin is the table builder.
- **Open a file or a folder.** A single file with no folder starts with the tree folded.

## Architecture rules

- `stede-core`, `stede-markdown` and `stede-services` must never depend on a UI crate (ratatui, crossterm, iced, Slint). Front ends send key events in and draw the view model that comes back.
- Every key press maps to a named action (`edit.delete_line`, `pane.focus_right`). Keymaps are data, per mode and per focused pane. Built-in actions, user commands and plugin commands share one registry, and every action has a one-line description for the key cheat sheet.
- Prompts (new file, rename, confirm) and outputs (`:lint`, `:keys`, command output) open as modals. One close key, `Esc` by default, closes any modal. The modal stack lives in the core's layout model.
- Two parsers: tree-sitter-md for incremental raw-pane highlighting and text objects; comrak, on a worker thread, for the full tree with source positions that feeds the render model and scroll sync.
- Slow work (linters, file watching, parsing) runs on tokio and reports back on a channel. It must never block typing.
- Panes are open, folded to a strip at their edge, or off. Folding animates with a snap; animation can be turned off.

## Crates

| Crate | Holds |
| --- | --- |
| `stede-core` | Buffers (ropey), Vim engine, keymaps, commands, layout model, config |
| `stede-markdown` | Markdown document type: parsers, render model, outline |
| `stede-services` | File tree and watcher, linter hub, external commands |
| `stede-tui` | The `stede` binary: ratatui drawing, crossterm input, images |
| `stede-gui` | Placeholder, phase 4; not built by default |
| `stede-plugin` | Placeholder, phase 3 (Steel host); not built by default |

Planned crates and their reasons are in the "Recommended stack" table in `docs/architecture.md`. Add dependencies as a phase needs them, not ahead of time, and check each license.

## Conventions

- Every source file starts with `// SPDX-License-Identifier: GPL-3.0-or-later`.
- `unsafe_code` is forbidden workspace-wide.
- Screens are tested with `insta` snapshots on ratatui's `TestBackend`.
- Config is TOML, merged in layers: defaults, user file, `.stede/config.toml`, `.stede/local.toml`, profile, session `:set`. Unknown keys and errors report file, line and key.
- Wide characters count by display width (CJK, emoji) wherever text is aligned.
- Dependencies are pinned: `Cargo.lock` with `--locked`, `rust-toolchain.toml`, and actions by commit SHA. New releases wait 14 days; patch and security fixes may come in at once, except GitHub Actions, where every release waits 14 days (security updates still skip the wait). CI's `Dependency age` check (`scripts/dependency-age.py`) enforces this on every PR. Add crates to `[workspace.dependencies]`. See "Dependencies" in `CONTRIBUTING.md`.

## Commands

Tasks live in the `justfile`, and CI runs the same recipes. Setup and the full list are in `CONTRIBUTING.md`.

```sh
just setup                  # pinned toolchain and dev tools
just ci                     # everything CI runs: fmt, clippy, tests, docs, deny, layers, typos, MSRV
just dep-age                # 14-day rule on new dependency versions (network, Python 3.11+)
just fmt                    # cargo fmt --all
just layers                 # headless crates must not depend on UI crates
cargo run -p stede-tui      # runs `stede`
```

## Pull requests

Every PR Claude opens goes through the `stede-pr-review` skill (`.claude/skills/stede-pr-review/`). Round 1: review, fix, push. Round 2: review again, fix, push. Once CI is green, Claude squash-merges the PR itself and moves on. Effort scales with PR size, as the skill describes. Refine the skill when it misses something or flags noise.

## Roadmap

1. **Phase 0, spike:** prove the risky parts: comrak source positions driving scroll sync, kitty text sizing and Sixel in target terminals, key handling in Windows Terminal under WSL.
2. **Phase 1, Reader:** open a folder, file tree, rendered pane, scroll sync, search, themes, config layers. Gate: the GFM spec files render correctly. Exit deliverables: a manual UX test plan for the reader and a first draft of end-user docs for what phase 1 ships.
3. **Phase 2, Writer:** Vim engine, keymaps, save, undo tree, linter adapter, clipboard (OSC 52). Gate: the user uses it daily for their own docs.
4. **Phase 3, Extend:** external commands and hooks, profiles, more flavors, templates, Steel plugins starting with the table builder. Gate: core API frozen.
5. **Phase 4, Native window.**

Current state: phase 0 scaffold. Nothing but crate stubs exists yet.

## Still open

- AI hooks: is piping to an outside CLI enough, or should there be a built-in panel? Deferred.
- GPL-3.0-only or GPL-3.0-or-later? The scaffold assumes "or later".
