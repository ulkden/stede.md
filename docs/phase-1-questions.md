# Phase 1: questions for the owner

Collected during the autonomous phase 1 run. Each entry gives the question, the options, and what was done meanwhile. The owner answered them all on 2026-11-24; each **Answer** follows its question.

1. **MSRV policy.** Dependencies keep raising the minimum Rust version: ratatui 0.30.2 needs 1.88, and comrak 0.59 needs 1.89. Meanwhile each bump has followed what the dependency needs. Should the MSRV track "pinned toolchain minus N releases" instead, or be dropped for a binary-only app?

    **Answer:** follow the dependencies. Raise the MSRV only when a dependency needs it, and keep CI's MSRV job so it never drifts silently.
2. **comrak 1.x.** 1.0.0 was released 2026-11-17, so it's eligible from 2026-12-01. Step 3 starts on 0.59.0, and Dependabot will propose the major bump. Is moving to 1.x once it's eligible fine?

    **Answer:** move to comrak 1.x now rather than wait. On 2026-11-24 the owner chose 1.2.0 (released 2026-11-23) over 1.0.0 and 1.1.0, accepting the risk of a release under 14 days old. This is a one-off owner waiver: the upgrade PR carries the `dependency-age-exempt` label and says so (see "Dependencies" in `CONTRIBUTING.md`).
3. **Unicode-DFS-2016 on the license allow list.** comrak's dependency `finl_unicode` uses it. It's the permissive predecessor of Unicode-3.0, which is already allowed, and the FSF lists it as GPL-compatible. It was added to `deny.toml` with that reason in step 3's PR. Do you want it reviewed or kept?

    **Answer:** keep it on the allow list, with the reason in `deny.toml`.
4. **The GFM spec in the repo.** The phase gate needs the spec's examples, so `gfm-spec.txt` (CC BY-SA 4.0) is vendored as test data under `crates/stede-markdown/tests/spec/`, with its source and license in a README there. It's never built into the binary, and BY-SA 4.0 is one-way compatible with GPLv3. The alternative is downloading it at test time, pinned by checksum. Is vendoring OK?

    **Answer:** keep it vendored.
5. **What "the GFM spec renders correctly" means for the gate.** Interim definition: for all 672 examples, no panics, valid source ranges, no text lost between comrak and the render model, and comrak's HTML (with our options) matching the spec's, apart from 17 listed exceptions that don't affect what the pane shows. The terminal drawing of each example gets checked by snapshots in step 6. Does that meet what you meant?

    **Answer:** accepted as written; the definition is now in `docs/phase-1.md` under the gate.
6. **`[keys.modal]` in the design doc is written the other way round.** It reads `close = "<Esc>"`, which is action = keys, while every other keys table is keys = action (`"<leader>p" = "pane.toggle rendered"`). Step 4 uses keys = action everywhere, so it's `"<Esc>" = "modal.close"`. Should the doc be fixed to match, or do you want the action-first form for modals?

    **Answer:** keys-first everywhere. `docs/architecture.md` is fixed to match.
7. **Settings that aren't built yet.** A config that names a planned setting (`[ui.animation]`, `[editor]`, `[modal]`, `[help]`, `[[commands]]`, `preset = "plain"`) gets a "not built yet" note rather than an "unknown key" error, so a config written from the design doc doesn't look broken. Is that the behavior you want?

    **Answer:** keep the notes.
8. **Dotfiles in the tree.** The tree hides dotfiles and anything `.gitignore` leaves out, as most editors do, so `.stede/` itself doesn't show. A toggle (a key, or `[tree] hidden = true`) would come later. Is hiding them the right default?

    **Answer:** hide them by default, and add a key and a `[tree]` setting to show hidden and ignored files. That's a follow-up PR.
9. **What the watcher watches.** Each folder the tree has listed is watched, but not recursively, so a huge `node_modules` costs nothing until you open it. Changes to folders you've never opened aren't seen until you open them, which is when they're listed anyway. Is that OK?

    **Answer:** watch the whole tree recursively, skipping ignored paths, so changes in folders that haven't been opened are seen too. That's a follow-up PR.
10. **Raw-pane highlighting without tree-sitter-md, for now.** CLAUDE.md gives raw-pane highlighting to tree-sitter-md. Its strength is fast incremental reparsing while you type, and the reader doesn't edit. So step 6a colors raw lines from comrak's block ranges (headings, code, quotes, rules, front matter), and tree-sitter-md arrives with the editor in phase 2, along with its text objects. That keeps a C-compiled dependency out of the reader. Is deferring it OK?

    **Answer:** defer tree-sitter-md to phase 2, where it arrives with the editor.
11. **The render model's types moved into `stede-core`.** That's `stede_core::render`; `stede-markdown` builds the model and re-exports the types. The services carry a parsed document back from a worker, and `ServiceEvent` lives in the core, which can't depend on `stede-markdown`. `docs/architecture.md` already says the core holds the render model, but the CLAUDE.md crate table still lists it under `stede-markdown`. I haven't edited CLAUDE.md; should its table say the core owns the types?

    **Answer:** CLAUDE.md's crate table is updated: the core holds the render model types, and `stede-markdown` builds them.
12. **The search prompt is on the status line, not in a modal.** CLAUDE.md says prompts open as modals. `/` and `?` type on the status line instead, as in Vim, so matches stay visible and light up in both panes as you type. A modal over the panes would hide them. `Esc` still closes it, and a modal that opens over it (a config reload's problems) drops the pattern so the modal's keys work. Should search count as an exception to the modal rule, written into CLAUDE.md, or should it become a modal anyway?

    **Answer:** keep the status-line prompt as an exception, now written into CLAUDE.md's modal rule.
