<!--
Adapted from existential-birds/beagle, plugins/beagle-rust/skills/tokio-async-code-review/SKILL.md
at commit d1a74899fbfec74974d1818e4cac7c3d54d44b65. Licensed under Apache-2.0 (see LICENSE-Apache-2.0).
Modified for STEDE.md: condensed and re-scoped to an interactive editor whose input loop must never block.
-->

# Async, tokio and concurrency checklist

The STEDE model: the input loop owns state. Slow work (comrak parses, linters, the file watcher, external commands) runs on tokio or worker threads and sends results back on a channel that wakes the loop. Review against that model.

## Before reporting

- Note the tokio version and features from `Cargo.toml` (workspace or crate).
- Name the runtime flavor (`multi_thread` or `current_thread`) and where it is built.
- List the blocking calls in async or loop code (`std::fs`, `std::process::Command::output`, `std::thread::sleep`, heavy CPU work) as `path:line`, or state "none found".

## Checklist

- [ ] **Nothing blocks the input loop:**
  - no `block_on` inside the loop or inside async code
  - no `std::thread::sleep`
  - no sync file or process IO on the loop thread
  - CPU-heavy work (a full parse, a big tree walk) goes to `spawn_blocking` or a worker thread
- [ ] **No lock held across `.await`:** use `tokio::sync::Mutex` if it must be, or drop the guard first. `std::sync::Mutex` is fine for short sections with no `.await`.
- [ ] **Stale results are dropped, not applied:**
  - A parse or lint result for an older buffer version doesn't overwrite newer state. Look for a version or generation number on each result.
  - Rapid typing doesn't queue unbounded work. Debounce, or cancel the superseded job.
- [ ] **Bounded channels** wherever a producer can outrun the loop (the watcher on a big folder, linter output). Name the backpressure policy: drop, coalesce or block the producer, but never block the loop.
- [ ] **Channel ends handled:** `SendError` means the receiver has gone, which is normal at shutdown, so don't panic. A `broadcast` `Lagged` is handled.
- [ ] **Tasks are owned:** a `JoinHandle` is tracked or the task has a shutdown signal (`CancellationToken`, a closed channel). A `JoinError` from a panicked task is reported, not unwrapped.
- [ ] **Cancellation safety:** `tokio::select!` branches don't lose data when another branch wins.
- [ ] **Timeouts** on external commands and LSP or linter processes that could hang. Child processes are killed on drop or shutdown.
- [ ] **Clean shutdown:** quitting stops the watcher and linters, waits briefly for them, and restores the terminal even when a task is stuck.
- [ ] **Shared state** across threads has correct `Send`/`Sync` bounds. Prefer message passing to shared mutable state.
- [ ] **No feedback loops:** work the app does mustn't trigger itself. Reading a watched folder raises access events, and saving a watched file raises a change. Check which event kinds a watcher acts on, and test that a listing (or a write the app makes) doesn't ask for another (Critical: it spins in the background).

## Severity guide

- **Critical:** blocking IO or sleep on the input loop; a guard held across `.await` that can deadlock; an unbounded queue a big folder can fill.
- **Major:**
  - stale results applied over newer state
  - a dropped `JoinHandle` that hides failures
  - no timeout on an external process
  - the wrong mutex type
  - a `select!` that isn't cancellation-safe
- **Minor:** spawning for trivial work; channel sizes with no rationale.
- **Informational:** `JoinSet` or `CancellationToken` suggestions; structured-concurrency refactors.

## Don't flag

`std::sync::Mutex` with no `.await` inside · cloning an `Arc` before `spawn` · `tokio::spawn` without a join when a shutdown signal exists · channel capacity 1 used as a handoff · `current_thread` runtime if it fits.
