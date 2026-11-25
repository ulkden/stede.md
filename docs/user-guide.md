# STEDE.md user guide

> **Draft.** This covers what phase 1 (the reader) ships. STEDE.md can't edit files yet; writing arrives in phase 2.

STEDE.md is a Markdown reader for the terminal. It shows a file's raw text and its rendered view side by side, with a file tree next to them. It uses Vim keys by default, and every key, color and setting can be changed.

## Starting

```sh
stede                 # the current folder
stede notes/          # a folder: the tree opens beside the reader
stede README.md       # one file: the tree starts folded
stede --probe         # check what this terminal supports
stede --help
stede --version
```

The screen has three panes and a status line:

```text
┌ Raw: README.md ──────┐┌ Rendered ────────────┐┌ Files ──────┐
│1 # Title             ││Title                 ││ docs/       │
│2                     ││━━━━━                 ││ README.md   │
│3 Some *text*.        ││                      ││             │
│                      ││Some text.            ││             │
└──────────────────────┘└──────────────────────┘└─────────────┘
 NORMAL  README.md
```

- **Raw** shows the file as written, with line numbers, colored by block (headings, code, quotes, rules, front matter).
- **Rendered** shows it formatted: headings, lists and task lists, tables, code blocks, quotes, alerts (`> [!NOTE]`), links and footnotes. It's read-only.
- **Files** is the folder's tree. It hides dotfiles and anything `.gitignore` leaves out; `H` and `I` in the tree show them (its title then says `+hidden` or `+ignored`). Git's own `.git` folder always stays hidden.
- **The status line** shows the mode, keys you're partway through typing, the search count (`[3/12]`) and the open file. Messages and errors show there too.

The two reader panes scroll together. Scroll either one and the other follows to the same place in the document.

A pane can be **open**, **folded** to a thin strip at its edge, or **off**. Moving focus onto a folded pane opens it.

## Keys

`<leader>` is `Space` by default. `<C-x>` means Ctrl+x. Press `<leader>?` or `F1` for the cheat sheet. It lists every key, with your own bindings, in sections for where each one applies: anywhere, each pane, and windows. The keys for typing a search aren't on it; they're under [Search](#search).

### Anywhere (when no window is open)

| Keys | Does |
| --- | --- |
| `ZZ` or `ZQ` | Quit |
| `<C-h>`, `<C-l>` (or `<C-w>h`, `<C-w>l`) | Focus the pane to the left or right |
| `<leader>r`, `<leader>p`, `<leader>e` | Fold or open the raw pane, the rendered pane, the file tree |
| `<leader>z` | Fold every pane but this one; press again to bring the layout back |
| `<leader>n` | Line numbers on or off |
| `<leader>h` | Hide search highlights (`n` still works and lights them again) |
| `<leader>?` or `F1` | The key cheat sheet |
| `<leader>c` | The config problems found at start |
| `<leader>o` | Reopen the last output window |

### Raw and rendered panes

| Keys | Does |
| --- | --- |
| `j`, `k` (or `↓`, `↑`) | Scroll a line |
| `<C-d>`, `<C-u>` | Scroll half a page |
| `<C-f>`, `<C-b>` (or `PageDown`, `PageUp`) | Scroll a page |
| `gg`, `G` | Go to the top or the end. With a count, go to that line: `50G` |
| `/`, `?` | Search forward or backward |
| `n`, `N` | Next or previous match |
| `Enter` (rendered pane) | Show the top block's source in the raw pane, and move focus there (opening it if folded) |

Counts work as in Vim: `5j` scrolls five lines, and `3n` goes three matches on.

### File tree

| Keys | Does |
| --- | --- |
| `j`, `k` (or `↓`, `↑`) | Next or previous entry |
| `gg`, `G` | First or last entry |
| `l`, `Enter` or `→` | Open the file or folder; on an open folder, close it |
| `h` or `←` | Close the folder, or go to its parent |
| `H` | Show or hide dotfiles |
| `I` | Show or hide files that `.gitignore`, `.ignore` or git's excludes leave out |

### Windows (cheat sheet, config problems, output)

| Keys | Does |
| --- | --- |
| `Esc` | Close the top window |
| `<C-c>` | Close every window |
| `j`, `k` | Scroll |
| `<leader>?` or `F1` | The cheat sheet, over the window |

While a window is open, only these keys work; the others wait until it's closed.

## Search

Press `/` to search forward, or `?` to search backward, from the raw or the rendered pane. Type a pattern on the status line; matches light up in both panes as you type.

- `Enter` goes to the first match. `Esc` stops and brings back the search you had before.
- `Backspace` (or `<C-h>`) deletes a character, and `<C-u>` clears the pattern.
- An empty pattern (`/` then `Enter`) repeats the last search.
- `n` goes to the next match in the search's direction, and `N` the other way. When the search wraps past the end of the file, the status line says so.
- If you've scrolled away from the last match, `n` searches on from where you are.

Patterns are regular expressions ([Rust regex syntax](https://docs.rs/regex/latest/regex/#syntax)) with **smart case**. If a pattern has no capital letters, case doesn't matter; if it has one, case counts. Letters in escapes like `\W` don't count. A pattern that isn't a valid regex, such as `(x`, is searched as plain text, and the status line says so.

In the rendered pane, matches are found in the text as drawn. A pattern with Markdown markup in it (`**bold**`) lights only in the raw pane.

## Files

- Markdown files (`.md`, `.markdown`, `.mdown`, `.mkd`, `.mkdn`, `.mdx`, `.mdtext`) are rendered. Other text files are shown in the rendered pane as one code block.
- Files over 20 MiB and binary files don't open; the pane says why.
- When the open file changes on disk, it reloads and keeps your place. Folders shown in the tree update when files are added or removed.
- STEDE.md watches the whole folder you open, skipping what the tree hides (dotfiles and ignored folders, unless you show them). It watches at most 4,096 folders, and says so once if there are more; folders you open in the tree are always watched. On WSL, folders under `/mnt/` are polled, so there only the folders you open are watched.

## Configuration

Settings are TOML files, read in layers. Each later layer wins:

1. Built-in defaults
2. Your user file: `$XDG_CONFIG_HOME/stede/config.toml`, else `~/.config/stede/config.toml`
3. The project's `.stede/config.toml`, found by walking up from the opened path
4. The project's `.stede/local.toml` (for settings you don't commit)

Saving a config file applies it at once, with no restart.

A bad entry is reported with its file, line and key, and skipped. Everything else still applies. Problems found at start open in a window, and `<leader>c` shows them again. Settings that are planned but not built yet (such as `[ui] animation`) get a "not built yet" note rather than an error.

### Settings

```toml
flavor = "gfm"                 # or "commonmark" (also accepted as [markdown] flavor)

[theme]
name = "dark"                  # or "light" (a bare theme = "light" works too)

[ui]
tree_side = "right"            # or "left"

[tree]
show_hidden = false            # dotfiles (H toggles it)
show_ignored = false           # files .gitignore leaves out (I toggles it)

[editor.line_numbers]
mode = "absolute"              # or "off"; "relative" and "hybrid" show absolute for now

[keys]
leader = "space"               # any single key: ",", "\\", "<Space>"
timeout_ms = 1000              # how long to wait for the next key of a sequence (100–10000)
```

### Keys

Bindings go under a table per place, as `keys = "action"`:

| Table | Where it applies |
| --- | --- |
| `[keys.normal]` | Everywhere, when no window is open |
| `[keys.raw]`, `[keys.rendered]`, `[keys.tree]` | While that pane has focus |
| `[keys.modal]` | In any window (cheat sheet, problems, output) |
| `[keys.output]` | In output windows |
| `[keys.search]` | While typing a search pattern |
| `[keys.probe]` | In `stede --probe` |

```toml
[keys.normal]
"<leader>t" = "pane.toggle tree"
"<C-q>" = "app.quit"

[keys.raw]
"J" = "view.half_down"

[keys.search]
"<C-c>" = "search.cancel"
```

Keys use Vim notation: `<C-x>` (Ctrl), `<A-x>` (Alt), `<S-Tab>`, `<CR>`, `<Esc>`, `<Space>`, `<leader>`, `<F1>`. A sequence is written as a string of keys, such as `gg` or `<C-w>h`. The actions you can bind are listed below, under [Actions](#actions).

In `[keys.search]`, keys you bind no longer type into the pattern. If you start a sequence binding (`jk`) and don't finish it, the keys you typed go into the pattern.

### Colors

Start from a built-in theme and override slots:

```toml
[theme]
name = "dark"

[theme.colors]
"md.heading" = "#ff9e64"
"ui.accent" = "bright-blue"
"search.match" = "#3d59a1"
```

A color is `#rrggbb`, `#rgb`, `default` (the terminal's own), or an ANSI name (`red`, `bright-blue`, and so on). On terminals without true color, colors are shown as the nearest of 256 or 16. True color is used when `COLORTERM` is `truecolor` or `24bit`.

| Slot | Colors |
| --- | --- |
| `ui.accent` | Focused pane border and title, window borders, keys in the cheat sheet, Note labels |
| `ui.border` | Borders of unfocused panes |
| `ui.dim` | Secondary text: actions in the cheat sheet, window footers |
| `ui.strip.fg`, `ui.strip.bg` | A folded pane's strip |
| `ui.status.fg`, `ui.status.bg` | The mode badge in the status line |
| `ui.linenr` | Line numbers in the raw pane |
| `md.heading` | Headings, and Warning and Caution labels |
| `md.code`, `md.code.bg` | Code (and Tip labels), and the band behind code blocks in the rendered pane |
| `md.quote` | Block quotes, the alerts' bar, and Important labels |
| `md.rule` | Thematic breaks and table borders |
| `md.meta` | Front matter and raw HTML |
| `search.match`, `search.current` | Behind search matches, and behind matches on the current match's line |

## Actions

| Action | Does |
| --- | --- |
| `app.quit` | Quit |
| `help.keys` | Open the key cheat sheet |
| `config.problems` | Show the config problems found at start |
| `pane.focus_left`, `pane.focus_right` | Move focus to the pane on the left or right, opening it if folded |
| `pane.focus_one` | Fold every pane but this one; again to restore the layout |
| `pane.toggle raw`, `pane.toggle rendered`, `pane.toggle tree` | Fold or open a pane |
| `line_numbers.toggle` | Show or hide line numbers in the raw pane |
| `view.down`, `view.up` | Scroll a line |
| `view.half_down`, `view.half_up` | Scroll half a page |
| `view.page_down`, `view.page_up` | Scroll a page |
| `view.top`, `view.bottom` | Go to the top or the end (with a count, to that line) |
| `rendered.jump` | Show the top rendered block's source in the raw pane |
| `tree.down`, `tree.up`, `tree.top`, `tree.bottom` | Move the tree's selection |
| `tree.open`, `tree.close` | Open or close the selected file or folder |
| `tree.toggle_hidden`, `tree.toggle_ignored` | Show or hide dotfiles, or ignored files, in the tree |
| `search.forward`, `search.backward` | Start typing a search |
| `search.next`, `search.prev` | Next or previous match |
| `search.off` | Hide search highlights |
| `search.confirm`, `search.cancel` | Search for the pattern, or stop typing it (`[keys.search]`) |
| `search.backspace`, `search.clear` | Delete a character of the pattern, or all of it (`[keys.search]`) |
| `search.insert <text>` | Type text into the pattern, as a key bound in `[keys.search]` (`"<C-t>" = "search.insert TODO"`) |
| `modal.close`, `modal.close_all` | Close the top window, or every window |
| `modal.scroll_down`, `modal.scroll_up` | Scroll the top window |
| `modal.reopen_last` | Reopen the last output window |
| `probe.quit` | Leave `stede --probe` (`[keys.probe]`) |

## Checking your terminal

`stede --probe` prints a sample for each feature STEDE.md can use (true color, text sizing, images), then echoes each key as STEDE.md sees it. That's useful for finding out what a key combination sends in your terminal, or why a binding doesn't fire. Press `qq` to leave.

## Not yet

These are planned but not built: editing and saving (phase 2), a linter adapter, the Obsidian and MkDocs flavors, images in the rendered pane, mouse support, pane animation, and plugins.
