# STEDE.md developer tasks. Run `just` to list them.
# CI calls these same recipes, so `just ci` passing locally means CI should too.

set shell := ["bash", "-euo", "pipefail", "-c"]

# Pinned dev tools: the one place their versions live. CI reads them from here
# through .github/actions/setup. Bump them by hand in the monthly cycle
# (CONTRIBUTING.md). Only `just` itself is also pinned in that action, and CI
# fails if the two differ.
just_version := "1.59.0"
deny_version := "0.20.2"
insta_version := "1.49.0"
typos_version := "1.52.0"

# Tools CI installs, in taiki-e/install-action's naming.
ci_tools := "cargo-deny@" + deny_version + ",typos@" + typos_version

# Minimum supported Rust version, read from `rust-version` in Cargo.toml.
msrv := `sed -n 's/^rust-version = "\(.*\)"/\1/p' Cargo.toml`

# Crates that hold no UI code, and the UI crates they must never pull in.
headless_crates := "stede-core stede-markdown stede-services"
# Terminal UI, GUI and windowing crates; `tui` is ratatui's old name.
ui_crates := "ratatui|tui|crossterm|termion|termwiz|cursive|iced|slint|egui|eframe|gpui|winit|tao"

[private]
default:
    @just --list

# Install the pinned toolchains and dev tools
setup:
    rustup toolchain install
    rustup toolchain install {{ msrv }} --profile minimal
    cargo install --locked just@{{ just_version }} cargo-deny@{{ deny_version }} cargo-insta@{{ insta_version }} typos-cli@{{ typos_version }}

# Run every check CI runs, except dep-age (needs network and Python 3.11+)
ci: fmt-check lint test doc layers deny typos msrv

# Format all code
fmt:
    cargo fmt --all

# Check formatting
fmt-check:
    cargo fmt --all --check

# Clippy with warnings as errors
lint:
    cargo clippy --workspace --all-targets --locked -- -D warnings

# Run all tests
test:
    cargo test --workspace --locked

# Build docs with warnings as errors
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked

# Licenses, advisories, bans and sources
deny:
    cargo deny --locked check

# Advisories only (what the daily audit runs)
audit:
    cargo deny --locked check advisories

# Check that dependency versions new since origin/main are 14+ days old (needs network)
dep-age *args:
    python3 scripts/dependency-age.py {{ args }}

# Pin too-new crates in Cargo.lock to their newest release that is 14+ days old
dep-settle:
    python3 scripts/dependency-age.py --fix

# Spell check code and docs
typos:
    typos

# Check the workspace builds on the minimum supported Rust version
msrv:
    cargo +{{ msrv }} check --workspace --all-targets --locked

# Fail if a headless crate depends on a UI crate
layers:
    #!/usr/bin/env bash
    set -euo pipefail
    status=0
    for crate in {{ headless_crates }}; do
        # Every feature and every target, so optional and platform-only deps count too.
        # Run cargo tree on its own: inside the `if` below, a failure would read as "no hits".
        if ! tree=$(cargo tree --locked -p "$crate" --all-features --target all --edges normal,build,dev --prefix none --format '{p}'); then
            echo "error: cargo tree failed for $crate" >&2
            exit 1
        fi
        # grep exits 1 for "no match" (good) and 2 for an error, which must not pass.
        rc=0
        hits=$(grep -E '^({{ ui_crates }})([-_][a-z0-9_-]+)? v' <<<"$tree") || rc=$?
        if [ "$rc" -gt 1 ]; then
            echo "error: grep failed checking $crate; check ui_crates in the justfile" >&2
            exit 1
        fi
        if [ "$rc" -eq 0 ]; then
            echo "error: $crate must not depend on a UI crate:" >&2
            echo "$hits" | sort -u >&2
            status=1
        fi
    done
    exit $status

# Review pending insta snapshots
snap:
    cargo insta review

# Use the repo's git hooks (format and spell check before each commit)
install-hooks:
    git config core.hooksPath .githooks
