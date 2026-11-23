# Contributing to STEDE.md

The design is in [`docs/architecture.md`](docs/architecture.md) and the rules that must not drift are in [`CLAUDE.md`](CLAUDE.md). This file covers the local setup, the checks, and how dependencies are kept up to date.

## Setup

You need [rustup](https://rustup.rs) and Python 3.11+ (for `just dep-age`; macOS's system `python3` is too old, so use Homebrew's). The toolchain is pinned in `rust-toolchain.toml`, and rustup picks it up inside the repo.

```sh
cargo install --locked just   # task runner; `just setup` then installs the pinned version
just setup                    # pinned toolchain, MSRV toolchain, just, cargo-deny, cargo-insta, typos
just install-hooks                   # optional: fmt and spell check before each commit
```

## Checks

`just` lists every task. The ones you will use most:

| Task | What it does |
| --- | --- |
| `just ci` | Everything CI runs except `dep-age`. Run it before you push |
| `just fmt` | Format the code |
| `just lint` | Clippy, warnings as errors |
| `just test` | All tests |
| `just snap` | Review pending `insta` snapshots |
| `just deny` | Licenses, advisories, bans and sources |
| `just layers` | Fail if `stede-core`, `stede-markdown` or `stede-services` depends on a UI crate |
| `just msrv` | Build on the minimum supported Rust version (`rust-version` in `Cargo.toml`) |
| `just dep-age` | Check that every dependency version new since `origin/main` is 14+ days old. Run it when you change dependencies |
| `just dep-settle` | Pin too-new crates in `Cargo.lock` to their newest release that is 14+ days old |

CI (`.github/workflows/ci.yml`) runs the same recipes on Linux and macOS, with the same tool versions and MSRV, all read from the `justfile` and `Cargo.toml`. A separate daily job (`audit.yml`) checks the committed `Cargo.lock` against the RustSec advisory database.

## Dependencies

### Everything is pinned

| What | Pinned in | Updated by |
| --- | --- | --- |
| Crates | `Cargo.lock` (committed); CI and `just` always pass `--locked` | Dependabot (`cargo`) |
| Rust toolchain | `rust-toolchain.toml` | Dependabot (`rust-toolchain`) |
| GitHub Actions | Full commit SHA, version in a comment | Dependabot (`github-actions`) |
| Dev tools (just, cargo-deny, cargo-insta, typos) | `justfile` only; CI reads it through `.github/actions/setup`. `just` is also pinned in that action, and CI fails if the two differ | By hand, in the monthly pass |

Declare third-party crates once, in `[workspace.dependencies]` in the root `Cargo.toml`, with a full `major.minor.patch` version. Member crates then use `name.workspace = true`. `Cargo.lock` is the real pin. Use an exact `=` requirement only for a crate that is known to break semver, and leave a comment saying why.

### The 14-day rule

A new release has to be **14 days old** before we take it. Most broken or hijacked releases are caught and yanked within that window.

- **Minor and major releases** (crates, toolchain, actions) wait 14 days. Dependabot's `cooldown` enforces this, so a PR never shows up for a release that is too new.
- **Patch releases** (bug fixes) to crates and the toolchain can come in at once. Dependabot proposes them without a cooldown. GitHub Actions is the exception: Dependabot only supports one flat cooldown there, so action patches wait 14 days too. Actions only run in CI and never ship in `stede`.
- **Security fixes** come in at once. Dependabot security updates skip the cooldown, and the daily audit fails as soon as an advisory matches `Cargo.lock`.

If you add a new dependency or bump one by hand, follow the same rule. Pick a release that is at least 14 days old unless it is a patch or security fix. Then run `just deny` to check its license.

**CI enforces the rule.** The `Dependency age` workflow (`scripts/dependency-age.py`) checks every version a PR introduces:
- crates in `Cargo.lock`, direct and transitive
- the toolchain
- GitHub Actions
- the dev tools in the `justfile`

A version passes if it is 14+ days old. Crates, the toolchain and dev tools also pass on a patch bump; actions don't, matching their flat Dependabot cooldown. The check also fails on loose pins:
- a `stable` toolchain channel
- an action not pinned to a commit SHA, or a `docker://` action not pinned by `@sha256` digest
- a version comment that doesn't match its SHA
- an action tag with no GitHub release (commit dates can be backdated, so they don't prove age)

The check matters most for the toolchain, where Dependabot can't apply its cooldown because the registry gives no publish date.

When you add or update crates, run `just dep-settle`. It pins every too-new crate in `Cargo.lock` to its newest release that is 14+ days old and semver-compatible, then runs the check. A crate it can't settle is listed as `stuck`, usually because something requires that exact version; wait for the eligible date or pick an older version of what pulls it in.

A PR that is too new fails until its eligible date, which the check prints. Re-run the job after that date. For a security fix that isn't a patch bump, add the `dependency-age-exempt` label and say why in the PR. The label waives only "too new"; pinning problems still fail. Run it locally with `just dep-age`, which needs Python 3.11+ and network access.

### The update cycle

- **Weekly (Monday):** Dependabot opens grouped PRs: one for crate patches, one for crate minors, one for actions, and one PR per major bump or toolchain release. Read the changelogs, let CI run, then merge. A major or toolchain bump may also need code changes, or an MSRV note if a crate raises its own `rust-version`.
- **Monthly (first Monday):** check the dev tools in the `justfile` for releases older than 14 days and bump them there. If `just` itself changes, also bump it in `.github/actions/setup/action.yml`; CI fails if they differ. Then run `just setup && just ci`. Also run `cargo update --dry-run` to spot anything Dependabot has missed.
- **Any time:** a security PR or a failing audit takes priority. Merge the fix once CI passes.
