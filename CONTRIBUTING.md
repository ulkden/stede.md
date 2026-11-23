# Contributing to STEDE.md

The design is in [`docs/architecture.md`](docs/architecture.md) and the rules that must not drift are in [`CLAUDE.md`](CLAUDE.md). This file covers the local setup, the checks, and how dependencies are kept up to date.

## Setup

You need [rustup](https://rustup.rs). The toolchain is pinned in `rust-toolchain.toml`, and rustup picks it up inside the repo.

```sh
cargo install --locked just@1.59.0   # task runner
just setup                           # pinned toolchain, MSRV toolchain, cargo-deny, cargo-insta, typos
just install-hooks                   # optional: fmt and spell check before each commit
```

## Checks

`just` lists every task. The ones you will use most:

| Task | What it does |
| --- | --- |
| `just ci` | Everything CI runs. Run it before you push |
| `just fmt` | Format the code |
| `just lint` | Clippy, warnings as errors |
| `just test` | All tests |
| `just snap` | Review pending `insta` snapshots |
| `just deny` | Licenses, advisories, bans and sources |
| `just layers` | Fail if `stede-core`, `stede-markdown` or `stede-services` depends on a UI crate |
| `just msrv` | Build on the minimum supported Rust version (`rust-version` in `Cargo.toml`) |

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

### The update cycle

- **Weekly (Monday):** Dependabot opens grouped PRs: one for crate patches, one for crate minors, one for actions, and one PR per major bump or toolchain release. Read the changelogs, let CI run, then merge. A major or toolchain bump may also need code changes, or an MSRV note if a crate raises its own `rust-version`.
- **Monthly (first Monday):** check the dev tools in the `justfile` for releases older than 14 days and bump them there. If `just` itself changes, also bump it in `.github/actions/setup/action.yml` and the setup line above. Then run `just setup && just ci`. Also run `cargo update --dry-run` to spot anything Dependabot has missed.
- **Any time:** a security PR or a failing audit takes priority. Merge the fix once CI passes.
