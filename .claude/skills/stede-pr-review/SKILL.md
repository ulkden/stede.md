---
name: stede-pr-review
description: Review a STEDE.md pull request in two rounds (review, fix, review, fix), then merge once CI is green. Combines the built-in /code-review bug pass with STEDE's project rules and a Rust/tokio checklist. Use every time Claude opens a PR in this repo, or when asked to review one.
argument-hint: "[PR number] [round 1|2]"
---

# STEDE.md PR review

This is the review loop for every PR Claude opens in this repo. It is ours to refine: when a round misses something or flags noise, fix this skill (or its references) in the same PR or the next one.

## The loop

1. Open the PR.
2. **Round 1:** run the review below. Fix the findings, push, and wait for CI.
3. **Round 2:** run the full review again on the updated PR. This is a full review, not just a check of round 1's fixes. Fix the findings, push, and wait for CI.
4. **Merge:** once CI is green after round 2, run `gh pr merge <n> --squash --delete-branch`. Then run `git switch main && git pull --ff-only` and carry on with the next task.

There is no round 3. The exception is a **Critical** finding in round 2: re-read its fix once more (that one spot, not a full round) before merging. Never merge with CI red or pending.

Each round ends with **one PR comment** (`gh pr comment <n> --body-file …`) using the format under Output. That leaves a record of what was found, what was fixed, and what was left and why.

## One round

### 1. Context and size

```sh
gh pr view <n> --json number,title,baseRefName,headRefName,additions,deletions,files
gh pr diff <n>
```

Pick the effort level from the PR's size and risk. Don't count `Cargo.lock`, `*.snap` or `LICENSE*` toward the size.

| Level | When |
| --- | --- |
| `medium` | 200 changed lines or fewer, **or** only docs, CI, config or dependency bumps |
| `high` | Anything else, and always for: concurrency (tokio, channels, threads, the event loop), parsers or scroll sync, keymaps and the action registry, config layering, the layout and modal model, a new crate or dependency, or a public API change between crates |

State the level and the reason in the round's PR comment.

### 2. Gate: the tools first

Run `just ci` on the PR branch. Fix anything it reports before reviewing, and don't count those as findings: a review should not repeat what the compiler, clippy, cargo-deny, typos or `just layers` already catch. If `just ci` cannot run, say why in one line.

### 3. Pass A: general bugs

Invoke the built-in `code-review` skill with `<level> <n>`. Keep its findings for step 5.

### 4. Pass B: project and Rust checklists

Read the full diff and the whole functions around each change, not just the hunks. Apply:

- [references/stede-rules.md](references/stede-rules.md): **always.** These are the mistakes only this project can make.
- [references/rust.md](references/rust.md): **always** when `.rs` files change.
- [references/tokio.md](references/tokio.md): when the diff touches async code, tokio, channels, threads, the watcher, the linter hub or anything on the event loop.

### 5. Verify, merge and rank

Before reporting anything, apply [references/verification.md](references/verification.md) to every finding from both passes. Drop what fails it. Merge duplicates. Rank by severity:

| Severity | Meaning | Action |
| --- | --- | --- |
| Critical | Unsound, data loss, a panic on user input, blocks typing, breaks a settled decision in `CLAUDE.md` | Fix before merge |
| Major | A real bug or a broken project rule | Fix before merge |
| Minor | A small correctness or clarity issue | Fix unless the fix is out of scope; say why if not |
| Informational | Ideas, new code, new dependencies, refactors | Don't fix in this PR; list as follow-ups |

Report everything you find in one pass; don't hold findings back for round 2. Round 2 may still find new issues: the code has changed, and a second read catches more.

### 6. Fix

- Fix Critical, Major and the in-scope Minor findings. Follow `CLAUDE.md` and add or update tests for behavior you change. Run `just ci`.
- Commit as `Address review round <N>` with a short list of what changed, then push.
- If a finding is wrong on a second look, say so in the PR comment rather than silently dropping it.
- If a finding reveals a gap in this skill, update the skill.

## Output (the PR comment)

```markdown
## Review round <N> (<level>: <reason>)

| # | Severity | Where | Finding | Disposition |
| --- | --- | --- | --- | --- |
| 1 | Major | `crates/x/src/y.rs:42` | One-line statement of the defect | Fixed in <sha> / Won't fix: <why> |

**Follow-ups (Informational):** <one line each, or "none">

**Checks:** `just ci` passed · CI <green|pending>
```

If there are no findings, write "No findings" in place of the table and keep the Checks line.

## Dependabot PRs

Dependabot PRs aren't opened by Claude, so the two-round loop doesn't apply. In the weekly cycle, run one `medium` round focused on:
- the changelogs between the old and new versions
- breaking changes and MSRV bumps
- licenses (`just deny`)
- whether the 14-day rule holds (see `CONTRIBUTING.md`)

Merge when CI is green. Major bumps and toolchain bumps get a `high` round.

## Sources

`references/rust.md`, `references/tokio.md` and `references/verification.md` are adapted from [existential-birds/beagle](https://github.com/existential-birds/beagle) (Apache-2.0). Each file's header names the upstream commit and says what was changed. See [references/LICENSE-Apache-2.0](references/LICENSE-Apache-2.0).
