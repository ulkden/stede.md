<!--
Adapted from existential-birds/beagle, plugins/beagle-rust/skills/rust-code-review/SKILL.md
at commit d1a74899fbfec74974d1818e4cac7c3d54d44b65. Licensed under Apache-2.0 (see LICENSE-Apache-2.0).
Modified for STEDE.md: condensed, re-scoped to this workspace (edition 2024, MSRV 1.85, unsafe forbidden),
removed web/database/lock-free sections, and added the terminal-app notes.
-->

# Rust checklist

Workspace facts: edition **2024**, `rust-version` **1.85**, `unsafe_code = "forbid"`, clippy `all` warns and CI denies warnings. Don't flag anything clippy or the compiler already reports.

## Ownership and borrowing

- [ ] No `.clone()` used to silence the borrow checker where a borrow or restructure works, especially in hot paths (rendering, per-keystroke handling, per-line highlighting).
- [ ] Parameters take `&str` or `&[T]` rather than `String` or `Vec<T>` when they don't need ownership; `impl AsRef` and `Into` work for flexible APIs.
- [ ] No premature `.collect()` when the consumer accepts an iterator; `_or_else` variants when the fallback allocates.
- [ ] Ropes: work in rope slices and char/line indices. Don't convert the whole buffer to `String` on each edit or draw.
- [ ] Edition 2024: `-> impl Trait` captures all in-scope lifetimes. Use `+ use<'a>` only when a narrower capture is needed. `if let` temporaries drop earlier; check guard lifetimes.

## Error handling

- [ ] Recoverable errors return `Result`; no `unwrap()` or `expect()` on user input, file contents, config, terminal or IO results in non-test code (Critical when user input can trigger it).
- [ ] Errors carry context when they cross a crate or module boundary: which file, which key, which command.
- [ ] Libraries (`stede-core`, `stede-markdown`, `stede-services`) use typed errors (thiserror or a hand-written `Display`). The binary can use an opaque error at the top.
- [ ] `let _ = tx.send(..)` is fine when the receiver may be gone; a dropped `Result` that matters is not.
- [ ] The terminal is restored on every exit path: raw mode off, alternate screen left, cursor shown. That includes panics (a panic hook) and errors.

## Traits, types and API between crates

- [ ] Public types between crates are minimal and stable: no leaking of a dependency's types that would make its major bump our breaking change, unless that's intended.
- [ ] Use newtypes where mixing up integers is a real risk: byte offset vs char index vs display column vs line number.
- [ ] Derives fit the type (`Debug` on public types; `Copy` only for small value types). `#[must_use]` goes where ignoring the result is a bug.
- [ ] `From` and `Into` are lossless; use `TryFrom` for fallible conversions.
- [ ] Getters are `name()`, not `get_name()`. `as_` is cheap, `to_` may allocate, and `into_` consumes.

## Naming, docs and lints

- [ ] Standard casing; `is_`, `has_` and `can_` for booleans.
- [ ] `pub` items have `///` docs, especially core APIs a front end or plugin will call (Minor).
- [ ] Lint suppressions use `#[expect(..)]` with a reason, not a bare `#[allow(..)]`.

## Performance (only where it runs often)

- [ ] No per-frame or per-keystroke allocation of whole documents, whole trees or whole screens when a diff or a slice works.
- [ ] Use `Vec::with_capacity` when the size is known on hot paths.
- [ ] Don't flag allocations in startup, error paths or tests.

## Severity guide

- **Critical:** a panic reachable from user input or file contents; data loss (a save or rename writing the wrong file or a truncated buffer); terminal left broken on exit; blocking the input loop.
- **Major:** errors without context across boundaries; `.clone()` masking a design issue on a hot path; wrong index type (bytes vs chars vs columns) that miscounts on non-ASCII text; `panic!` for recoverable errors in library code.
- **Minor:** missing docs on public items; `String` where `&str` works; missing derives; suboptimal iterator chains.
- **Informational:** newtype or refactor ideas; unmeasured optimizations; `#[non_exhaustive]` and `#[must_use]` suggestions.

## Don't flag

`.clone()` or `unwrap()` in tests · `.expect("reason")` on a real invariant · `String` fields in structs · `todo!()` in phase-scaffold code · turbofish · `_`-prefixed unused names · `Arc::clone(&x)` · `std::sync::Mutex` for short sections with no `.await` inside · native `async fn` in traits · `LazyLock` / `OnceLock`.
