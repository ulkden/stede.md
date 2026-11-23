<!--
Adapted from existential-birds/beagle, plugins/beagle-rust/skills/review-verification-protocol/SKILL.md
at commit d1a74899fbfec74974d1818e4cac7c3d54d44b65. Licensed under Apache-2.0 (see LICENSE-Apache-2.0).
Modified for STEDE.md: condensed into one gate list and pointed at this repo's rules.
-->

# Verify every finding before reporting it

False positives waste a fix cycle and teach the loop to ignore findings. Each finding must pass these gates, in order:

0. **Quote what you judge.** Re-read the file at `path:line` now and quote the code (or the diff hunk) in your working notes. Never judge from the branch name, earlier context or memory. If the file and your memory differ, the file wins.
1. **Read the whole unit.** Read the full function, `impl` or item that holds the line, not just the hunk.
2. **Check usages.** For "unused" or "dead": a workspace-wide search (`rg`) for the symbol finds zero real uses, counting other crates, `#[cfg(test)]`, derives, trait impls and `From`/`Into` paths.
3. **Check the surroundings.** Look at callers, trait definitions, `#[cfg]` gates and error propagation that might make the pattern intentional. Name one place you checked, or write "none relevant".
4. **Check edition and API.** Edition 2024 applies: `unsafe {}` inside `unsafe fn` is required, as are `unsafe extern` and `#[unsafe(no_mangle)]`. RPIT captures all lifetimes. Check crate APIs against the version in `Cargo.lock`, not memory.
5. **Wrong, not style.** One sentence on why the code is incorrect, unsound or risky **for this project**. If both ways are valid, drop the finding.
6. **Check intent.** `CLAUDE.md`, `docs/architecture.md` and code comments may show the choice was deliberate. A deliberate choice that conflicts with a settled decision is still a finding; a deliberate choice within the rules is not.

## Common false positives

- An `unwrap()` in tests, or an `expect()` after a check that makes it unreachable
- `let _ = tx.send(..)` when the receiver may be gone
- Cloning before `spawn` to satisfy `'static`
- A trait method that looks unused but is required by the trait
- `.collect()` on small or startup-only iterators
- Something clippy already flags, or that the project deliberately suppressed with `#[expect]` and a reason

## Scope

- Review the diff and the code it directly touches. Don't ask for unrelated features or architecture changes.
- Requests for net-new code (new modules, new dependencies, new test suites, new abstractions) are **Informational**.
- Missing tests are **one** Minor finding listing what's untested.
