# Phase 1 manual UX test plan: the reader

This plan checks by hand what automated tests can't: how the reader looks and feels in real terminals. It covers everything phase 1 ships. The user guide (`docs/user-guide.md`) describes the expected behavior; where this plan and the guide disagree, one of them has a bug.

## Before you start

Build once from the repo root, and run everything from the root too, so the paths in the cases work:

```sh
cargo build --locked --release -p stede-tui
alias stede="$PWD/target/release/stede"
```

The fixtures in `docs/ux-test/fixtures/`:

| File | Exercises |
| --- | --- |
| `tour.md` | Every GFM block: front matter, headings, lists, tasks, code (with a tab), quotes, alerts, a table, a rule, HTML, a footnote |
| `wide.md` | CJK, emoji and tabs: in prose, in a table cell and in a code block |
| `long.md` | One very long source line (with `needle` near its end), then enough lines to scroll |
| `notes.txt` | A non-Markdown file |
| `guides/`, `guides/nested/` | Folders in the tree |
| `ignored.md`, `.hidden.md` | Files the tree should hide |

Some cases need a file you make on the spot; each says how.

**Terminals:** run the whole plan once in your main terminal. Then run the cases marked **★** in each other target terminal: Ghostty, Terminal.app, kitty, WezTerm, GNOME Terminal, Windows Terminal under WSL, and tmux inside one of them. Use `stede --probe` first in each one (see the "Phase 0 checks" table in `docs/phase-1.md`).

**Recording:** tick each case, or write what happened instead. Note the terminal, its size, and any config you had loaded. A screenshot helps for anything visual.

## 1. Starting

| # | Do | Expect | ✓ |
| --- | --- | --- | --- |
| 1.1 ★ | `stede docs/ux-test/fixtures` | Raw, rendered and file tree panes, with the tree on the right. One pane has the accent border (focus). No file is open yet, and the reader panes say so | |
| 1.2 ★ | `stede docs/ux-test/fixtures/tour.md` | The file opens in both reader panes, and the tree starts folded to a strip | |
| 1.3 | `stede docs/ux-test/fixtures/does-not-exist.md` | A clear error, not a crash or a blank screen | |
| 1.4 | `stede --help`, then `stede --version` | Usage text, then a version number; neither starts the UI | |
| 1.5 | `stede --bogus` | A short error naming the bad option | |
| 1.6 ★ | Open `tour.md`, then quit with `ZZ` and reopen and quit with `ZQ` | The terminal comes back clean each time: cursor shown, no leftover colors, the prompt in the right place | |

## 2. The panes and the layout

| # | Do | Expect | ✓ |
| --- | --- | --- | --- |
| 2.1 ★ | Open the fixtures folder. Move focus with `<C-h>` and `<C-l>`, then `<C-w>h` and `<C-w>l` | Focus moves one pane at a time; the focused pane's border and title use the accent color | |
| 2.2 | `<leader>r`, `<leader>p` and `<leader>e`, each twice | Each pane folds to a strip at its edge, its title reading down the strip, then opens again. The other panes take the room | |
| 2.3 | Fold the tree, then `<C-l>` from the rendered pane | Focusing the folded pane opens it | |
| 2.4 | `<leader>z` in the raw pane, then `<leader>z` again | Only the raw pane shows, then the layout comes back as it was | |
| 2.5 | Try to fold the last open pane | It refuses, with a message, instead of leaving an empty screen | |
| 2.6 ★ | Resize the terminal from wide to very narrow (about 20 columns) and very short (about 5 rows), and back | No crash, no garbled borders; panes shrink, and text rewraps. It recovers fully when the terminal is large again | |
| 2.7 | Set `[ui] tree_side = "left"` in `docs/ux-test/fixtures/.stede/local.toml` (see section 9), saved while STEDE.md is open | The tree moves to the left at once | |

## 3. The file tree

| # | Do | Expect | ✓ |
| --- | --- | --- | --- |
| 3.1 | Open the fixtures folder and look at the tree | Folders first, then files. No `ignored.md`, `.hidden.md` or `.gitignore` | |
| 3.1b | In the tree, `H`, then `I`, then both again | `H` shows `.hidden.md`, `.gitignore` and `.stede/` if present, never `.git`. `I` shows `ignored.md`. The title says `+hidden` and `+ignored` while they show, and open folders update too | |
| 3.1c | Set `[tree]` with `show_hidden = true` in `docs/ux-test/fixtures/.stede/local.toml` and save; press `H`; save the file again unchanged | Dotfiles show on save. `H` hides them, and saving again without changing the setting doesn't bring them back | |
| 3.2 | `j`, `k`, `gg` and `G`, plus the arrow keys | The selection moves and stays visible in a long list | |
| 3.3 | `l` (or `Enter`) on `guides/`, then on `nested/`; `h` on `deep.md`; `h` again | Folders open in place. `h` on a file goes to its folder, and on an open folder closes it | |
| 3.4 ★ | `l` on `tour.md` | The file opens in the reader panes, and the raw pane title shows its name | |
| 3.5 | `l` on `notes.txt` | The raw pane shows the text; the rendered pane shows it as one code block | |
| 3.6 | With the folder open, create a file in another shell: `touch docs/ux-test/fixtures/new.md`, then delete it | It appears in the tree within about a second, then goes | |
| 3.7 | On WSL, open a folder under `/mnt/c/` and repeat 3.6 there | The same, perhaps a little slower (it polls) | |
| 3.8 | Open a folder with thousands of files (`mkdir /tmp/many && touch /tmp/many/f{1..5000}.md`) | The tree fills without freezing the UI; scrolling stays smooth | |

## 4. The raw pane

| # | Do | Expect | ✓ |
| --- | --- | --- | --- |
| 4.1 ★ | Open `tour.md` | Line numbers. Headings, code, quotes, rules and front matter in their colors; plain text uncolored | |
| 4.2 | `<leader>n` twice | Line numbers go and come back, and the text rewraps to the width | |
| 4.3 ★ | `j`, `k`, `<C-d>`, `<C-u>`, `<C-f>`, `<C-b>`, `PageDown`, `PageUp`, `gg` and `G` | Each scrolls as named. `G` keeps the last page full, with no empty rows past the end | |
| 4.4 | `5j`, `3k`, `20G`, `1G` | Counts repeat; `20G` puts line 20 at the top, unless the last page is shorter | |
| 4.5 ★ | Open `long.md` and scroll through the long line with `j` | It wraps by words onto many rows, and `j` moves one row at a time through it. Its line number shows only on its first row | |
| 4.6 ★ | Open `wide.md` | CJK and emoji take two cells each and never split. Tabs become spaces in both panes (in prose, the table and the code block), and nothing spills past the border | |
| 4.7 | Edit `tour.md` in another editor and save, while it's open and scrolled down | It reloads within about a second and keeps your place | |
| 4.8 | Make a file over 20 MiB (`head -c 21000000 /dev/zero \| tr '\0' 'a' > /tmp/big.md`) and open it | The pane says it's too large. No hang | |
| 4.9 | Open a binary file (`/bin/ls`) | The pane says it looks binary | |
| 4.10 | Make a file with only one line and a file that's empty, and open each | Both show normally; no panic | |

## 5. The rendered pane

| # | Do | Expect | ✓ |
| --- | --- | --- | --- |
| 5.1 ★ | Open `tour.md` and read the rendered pane top to bottom | Headings stand out (level 1 and 2 underlined); emphasis, strong, strikethrough and inline code look different; links show their text | |
| 5.2 | Lists in `tour.md` | Bullets and numbers. Nested lists indent, and wrapped items line up under their text, not their bullet | |
| 5.3 | Tasks | Done and open tasks have different checkboxes | |
| 5.4 ★ | Code blocks | A colored band. The language shows, and the tab is spaces | |
| 5.5 | Quotes and alerts | A bar on the left. Each alert has its label (Note, Warning) in its own color, and the alert that opens with a list keeps its bullet on the first item | |
| 5.6 ★ | The table, at full width, then fold the tree and narrow the terminal | Aligned columns with straight borders. As the pane narrows, cells wrap; when it can't fit, the table falls back to plain rows rather than breaking | |
| 5.7 ★ | `wide.md`'s table | Borders stay straight with CJK and emoji in the cells | |
| 5.8 | The rule, the HTML and the footnote | A rule across the pane, the HTML shown quietly, and the footnote at the end with its number | |
| 5.9 | Front matter | Shown quietly, not as a heading or a rule | |

## 6. Scroll sync

| # | Do | Expect | ✓ |
| --- | --- | --- | --- |
| 6.1 ★ | In `tour.md`, scroll the raw pane with `j` | The rendered pane keeps the same block at its top | |
| 6.2 ★ | Focus the rendered pane and scroll it | The raw pane follows | |
| 6.3 | In `long.md`, scroll through the long paragraph from either pane | Both panes move smoothly through it, rather than jumping a whole paragraph at a time | |
| 6.4 | `G` in either pane, then `gg` | Both panes reach their ends together, even when the rendered pane is taller (the table) | |
| 6.5 | Scroll the rendered pane to the table, then `Enter` | The raw pane shows the table's source at its top, and focus moves to it | |
| 6.6 | Fold the raw pane, scroll the rendered pane a long way, then open the raw pane | The raw pane opens at the matching place, readable, not scrolled past its text | |
| 6.7 | Resize the terminal while scrolled halfway | Both panes stay at the same place in the document | |
| 6.8 | Open a short note that fits on screen (`printf '# A\n\n## B\n\n| x |\n|---|\n| 1 |\n' > /tmp/short.md`) | Both panes start at the top, and `j` doesn't send the rendered pane to its bottom | |

## 7. Search

| # | Do | Expect | ✓ |
| --- | --- | --- | --- |
| 7.1 ★ | In `tour.md`, `/code` | The status line shows `/code` with a cursor. Matches light up in both panes as you type | |
| 7.2 ★ | `Enter`, then `n` several times, then `N` | The first match is at the top of the raw pane, and the rendered pane follows. `[k/N]` counts on the status line; wrapping past the end says so | |
| 7.3 | `?list` then `Enter`, then `n` | Searches backward; `n` keeps going backward | |
| 7.4 | `/code` then `Esc` while a different search is active | Typing stops, and the earlier search and its highlights come back | |
| 7.5 | `/` then `Enter` | Repeats the last search | |
| 7.6 | `/Code` and `/code` | Smart case: `Code` finds only the capitalized one, `code` finds all | |
| 7.7 | `/(x` | Searches the text `(x` and says it wasn't a valid regex | |
| 7.8 | `/nothing-here` | "pattern not found" | |
| 7.9 | `<leader>h`, then `n` | The highlights go; `n` still works and lights them again | |
| 7.10 ★ | In `long.md`, `/needle` | The match deep in the long line is on screen and lit, in both panes | |
| 7.11 | In `wide.md`, `/東京` | Lit in both panes, with the table borders still straight | |
| 7.12 | Search, scroll far away with `G`, then `n` | Goes to the next match after where you are now, not after the old match | |
| 7.13 | Search from the rendered pane | The same as from the raw pane | |
| 7.14 | Type fast, use `Backspace` and `<C-u>` while typing | Every key lands; the highlights keep up | |

## 8. Keys, the cheat sheet and windows

| # | Do | Expect | ✓ |
| --- | --- | --- | --- |
| 8.1 ★ | `<leader>?` (and `F1`) | The cheat sheet lists every key in sections (anywhere, each pane, windows), each with a description. It scrolls with `j` and `k`, and `Esc` closes it | |
| 8.1b | With the cheat sheet open, `ZZ` and `<C-l>` | Nothing happens to the app or panes: only window keys work while a window is open | |
| 8.2 | Type `<C-w>` and wait | The status line shows the pending `<C-w>`. After the timeout (one second) it says `<C-w>` is not bound | |
| 8.3 | Type `3` then `Esc` | The count is dropped | |
| 8.4 | Press an unbound key (`x`) | "x is not bound" on the status line | |
| 8.5 | Open the cheat sheet, then `<C-c>` | Every window closes | |
| 8.6 | Close a window, then `<leader>o` | The last output window comes back | |
| 8.7 ★ | In Windows Terminal under WSL and in tmux: `<C-h>`, `<C-l>`, `<C-d>`, `<C-u>`, `F1` and `Esc` | They all work. Escape doesn't add lag; if a key misbehaves, use `stede --probe` to see what the terminal sends | |

## 9. Configuration

Use `docs/ux-test/fixtures/.stede/local.toml` (git ignores it). For each case, **replace** the file's contents with the case's lines and save while STEDE.md is open. Delete the folder afterwards.

| # | Do | Expect | ✓ |
| --- | --- | --- | --- |
| 9.1 | `[theme]` with `name = "light"` | The theme switches at once | |
| 9.2 | `[theme.colors]` with `"md.heading" = "#ff0000"` | Headings turn red in both panes | |
| 9.3 | `[keys]` with `leader = ","` | `,?` opens the cheat sheet, and Space no longer does | |
| 9.4 | `[keys.raw]` with `"J" = "view.half_down"` | `J` scrolls half a page in the raw pane, and the cheat sheet lists it | |
| 9.5 | `[keys.search]` with `"<C-c>" = "search.cancel"` | `<C-c>` stops typing a search | |
| 9.6 | A typo, `[ui]` with `tree_sid = "left"`, and a bad value, `[keys]` with `timeout_ms = 5` | A problems window names the file, the line and the key for each; the rest of the config still applies | |
| 9.7 | Fix the file and save | The problems clear | |
| 9.8 | `[ui]` with `animation = true` | A "not built yet" note, not an error | |
| 9.9 | `flavor = "commonmark"` | Tables and task lists in `tour.md` stop rendering as such (CommonMark has neither) | |
| 9.10 | Start with a broken user config (`~/.config/stede/config.toml`, or under `$XDG_CONFIG_HOME`) | It starts anyway, shows the problems, and `<leader>c` shows them again | |
| 9.11 | `TERM=xterm-256color COLORTERM= stede …` and `TERM=xterm COLORTERM= stede …` | Colors downgrade to 256 and 16 colors but stay readable | |

## 10. Feel

These have no single right answer; write down impressions.

- Is the default layout's split between raw, rendered and tree comfortable at your usual terminal size?
- Do the dark and light themes read well, in your terminal's own colors and in true color?
- Does anything feel slow: opening a big folder, scrolling a long file, typing a search?
- Is anything surprising, or missing from the cheat sheet?
- Where did you reach for a key that wasn't there?

## Reporting

For each failure, open an issue with:
- the case number
- the terminal and its size
- the steps
- what you saw, ideally with a screenshot
- the config, if one was loaded
