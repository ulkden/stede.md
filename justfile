# STEDE.md developer tasks. Run `just` to list them.
# CI calls these same recipes, so `just ci` passing locally means CI should too.

set shell := ["bash", "-euo", "pipefail", "-c"]

# Pinned dev tools. Bump them by hand in the monthly cycle (CONTRIBUTING.md)
# and keep them in step with the `install-action` step in .github/workflows/.
just_version := "1.59.0"
deny_version := "0.20.2"
insta_version := "1.49.0"
typos_version := "1.52.0"

# Minimum supported Rust version; must match `rust-version` in Cargo.toml.
msrv := "1.85.0"

# Crates that hold no UI code, and the UI crates they must never pull in.
headless_crates := "stede-core stede-markdown stede-services"
ui_crates := "ratatui|crossterm|iced|slint|egui|gpui"

[private]
default:
    @just --list

# Install the pinned toolchains and dev tools
setup:
    rustup toolchain install
    rustup toolchain install {{ msrv }} --profile minimal
    cargo install --locked just@{{ just_version }} cargo-deny@{{ deny_version }} cargo-insta@{{ insta_version }} typos-cli@{{ typos_version }}

# Run every check CI runs
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
        if hits=$(cargo tree --locked -p "$crate" --prefix none --format '{p}' | grep -E '^({{ ui_crates }})([-_][a-z0-9_-]+)? v'); then
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
