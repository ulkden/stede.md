# Phase 1: the reader

Goal (from `docs/architecture.md`): open a folder or a file and read it, with a file tree, a raw pane and a rendered pane that scroll together, search, themes and config layers. Writing comes in phase 2.

**Gate:** the GFM spec files render correctly. For all 672 examples in the vendored spec (`crates/stede-markdown/tests/gfm_spec.rs`), that means:
- no panics
- valid source ranges
- no text lost between comrak and the render model
- comrak's HTML, with our options, matching the spec's, apart from 17 listed exceptions that don't change what the pane shows

The terminal drawing is checked by snapshot tests, and the layout of every example at widths 1, 10, 40 and 100. Agreed with the owner on 2026-11-24.
**Exit deliverables:** a manual UX test plan for the reader, and a first draft of end-user docs for what this phase ships.

Each step is one PR through the `stede-pr-review` loop. Tick a step when its PR merges.

## Steps

- [x] **1. Core foundations** (`stede-core`, no new crates)
  - Keys in Vim notation, the action registry, keymaps as data (counts, sequences, timeout)
  - The layout model: open, folded or off panes, focus moves, `focus_one`, geometry
  - The modal stack, the key cheat sheet built from the live keymap, and the core loop with its view model
- [x] **2. Terminal shell** (`stede-tui`: ratatui, crossterm)
  - The event loop, drawing panes, strips, modals and the status line
  - Terminal restore on exit and on panic
  - `insta` snapshots on `TestBackend`
- [x] **3. Markdown render model** (`stede-markdown`: comrak)
  - A render model of blocks with style tokens and source line ranges, so scroll sync can map lines both ways
  - The GFM spec examples as a test suite: the phase gate (`crates/stede-markdown/tests/gfm_spec.rs`)
- [x] **4. Config and themes** (serde, toml)
  - Layers: defaults, user file, `.stede/config.toml`, `.stede/local.toml` (session `:set` needs the command line, phase 2; profiles are phase 3)
  - Errors carry file, line and key
  - Keymap overrides (including `[keys.probe]`, which the probe must load too; it uses the defaults until then), theme slots, and the dark default theme
- [x] **5. File tree and watcher** (`stede-services`: ignore, notify, tokio)
  - Open a folder or a file; respect `.gitignore`
  - The tree pane's keys
  - Watch for changes, polling on `/mnt/*` in WSL
  - Reload config live when a config file is saved (step 4 reads it once at start)
- [x] **6a. Opening files and the raw pane**
  - Files read and parsed off the input loop; reloaded when they change on disk
  - The raw pane, read-only for now, with line numbers and highlighting by block
  - Scrolling keys
- [x] **6b. The rendered pane and scroll sync**
  - The rendered pane drawing the render model: headings, lists, tasks, tables, code, quotes, alerts, links
  - Scroll sync both ways; Enter on a block jumps the raw pane to it
- [x] **7. Search**
  - `/` and `?`, `n` and `N`, smart case, all matches lit, in either pane
  - Patterns are regexes; one that doesn't parse is searched as plain text. Matches light as you type; `Esc` restores the search before; an empty pattern repeats the last. `<leader>h` clears the lights
  - The rendered pane lights matches in the text as drawn, so markup in a pattern (`**`) only lights in the raw pane
- [x] **8. Exit deliverables**
  - A manual UX test plan for the reader: `docs/ux-test/README.md`, with fixtures in `docs/ux-test/fixtures/`
  - A first draft of end-user docs: `docs/user-guide.md`

## Phase 0 checks that need real terminals

The phase 0 spike also asked for proof in real terminals, which CI can't give. Run `stede --probe` (`cargo run -p stede-tui -- --probe`) in each terminal. It prints a sample for each check and then echoes keys as STEDE.md sees them; `qq` (the `probe.quit` action) leaves. Tick what works:

| Check | Ghostty | Terminal.app | kitty | WezTerm | GNOME Terminal | Windows Terminal (WSL) | tmux |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Keys: `<C-w>`, `<C-h>`/`<C-l>`, `<F1>`, `<Esc>` arrive intact | | | | | | | |
| kitty text sizing (OSC 66) for headings | | | | | | | |
| Sixel or kitty graphics for images | | | | | | | |
| Truecolor | | | | | | | |
