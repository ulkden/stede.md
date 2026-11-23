# STEDE.md project rules

These checks come from `CLAUDE.md` and `docs/architecture.md`. Breaking a settled decision is **Critical**; breaking any other rule here is **Major**, unless the note says otherwise. When a rule here and `CLAUDE.md` disagree, `CLAUDE.md` wins: fix this file.

## Crate layering

- [ ] `stede-core`, `stede-markdown` and `stede-services` don't depend on a UI crate (ratatui, crossterm, iced, Slint, egui, GPUI), directly or through another crate. `just layers` checks the dependency graph. Here, also check that no UI types leak in through re-exports, generic bounds or `cfg` gates.
- [ ] The core takes key events in and returns a view model. Front ends don't reach into core state to draw, and the core doesn't know about cells, colors in terminal form, or crossterm key types. It uses its own key type.
- [ ] Dependencies run one way: `stede-tui` → `stede-services` / `stede-markdown` → `stede-core`. Nothing depends on `stede-tui`. `stede-gui` and `stede-plugin` stay placeholders and aren't added to `default-members`.
- [ ] The rendered pane is **read-only**. No path through the code edits a buffer from the rendered pane (Critical).

## Actions, keys and modals

- [ ] Every key press resolves to a **named action** (`edit.delete_line`, `pane.focus_right`). Behavior is never keyed directly on a key in the front end.
- [ ] Every new action is in the one shared registry, with a one-line description for the key cheat sheet.
- [ ] Keymaps are data, per mode and per focused pane. Defaults come from data, not `match` arms that users can't override.
- [ ] Prompts and outputs (new file, rename, confirm, `:lint`, `:keys`, command output) open as **modals** on the core's modal stack. One close key (`Esc` by default, configurable) closes any modal. No ad hoc popups.

## Responsiveness

- [ ] Slow work (linters, file watching, parsing, external commands, file I/O on large trees) runs on tokio or a worker thread and reports back on a channel. Nothing on the input-to-draw path blocks: no `std::fs` reads of user files, no subprocess waits, no full comrak parses (Critical if it can stall typing).
- [ ] The front end draws only when something changed. Animation frames draw only while something moves and go out as one synchronized update.
- [ ] Folding animation can be turned off, and the off path is tested.

## Markdown and rendering

- [ ] tree-sitter-md does incremental raw-pane highlighting and text objects. comrak, on a worker thread, builds the full tree with source positions for the render model and scroll sync. Don't swap these roles or add a third parser.
- [ ] The render model carries **source line positions** for every block, so scroll sync and "Enter jumps to source" keep working.
- [ ] Flavor handling follows the order GFM (default), CommonMark, Obsidian-style, MkDocs. GFM behavior doesn't regress when another flavor is added.

## Text and layout

- [ ] Anything that aligns text counts **display width** (CJK, emoji, combining marks), never `len()` or `chars().count()`. Check tables, the file tree, strips and badges, the cursor column, and wrapping. Look for byte or char counts used as columns.
- [ ] Narrow terminals and resizes don't panic: there's no subtracting widths without `saturating_sub`, and no slicing at a byte index that may not sit on a char boundary.

## Config

- [ ] Config is TOML, merged in layers: defaults, user file (`$XDG_CONFIG_HOME/stede/` or `~/.config/stede/`), `.stede/config.toml`, `.stede/local.toml`, profile, session `:set`. New settings fit into this merge order.
- [ ] Unknown keys and invalid values report **file, line and key**. No silent ignore, and no panic on bad config.
- [ ] A setting the current front end can't honor (font, font size in the terminal) shows a short note, not a silent no-op.

## Tests

- [ ] Screen changes have `insta` snapshots on ratatui's `TestBackend`. New `.snap` files are reviewed, not just accepted: look at them in the diff.
- [ ] Behavior changes in the core have unit tests in the core, not only through the TUI.
- [ ] Missing tests for changed behavior: report as **one** Minor finding listing what's untested.

## Repo conventions

- [ ] Every new `.rs` file starts with `// SPDX-License-Identifier: GPL-3.0-or-later`.
- [ ] No `unsafe`. It is forbidden workspace-wide, so the compiler catches it; check that nobody tries to relax the lint (Critical).
- [ ] New crates go in `[workspace.dependencies]` with a full version, and member crates use `.workspace = true`. Check:
  - The release is at least 14 days old, unless it's a patch or security fix.
  - The license is on the `deny.toml` allow list. Adding a license needs a reason in the PR.
  - The phase actually needs it now, per "Add dependencies as a phase needs them" in `CLAUDE.md`.
- [ ] The workflows and the `justfile` stay in step: tool versions match, and actions are pinned to a commit SHA with a version comment.
- [ ] Docs match the change. Update `CLAUDE.md` if a rule or command changed, `CONTRIBUTING.md` if the workflow changed, and `docs/architecture.md` if the design moved. Stale docs are Minor.
