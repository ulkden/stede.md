# STEDE.md

A fast, terminal-first Markdown reader and writer for macOS, Linux and WSL.

- Raw Markdown and a rendered view side by side, with a file tree on the right
- Panes fold to a strip at the edge and open again with one click or key
- Vim keys by default; every binding, color and setting is configurable
- GitHub Flavored Markdown first, with other flavors as profiles
- Bring your own linter through LSP or any CLI

**Status:** design done, build starting. Nothing usable yet. See [`docs/architecture.md`](docs/architecture.md) for the plan.

## Build

```sh
cargo run -p stede-tui -- [PATH]       # a folder or a Markdown file (loading files lands later in phase 1)
cargo run -p stede-tui -- --probe       # check what your terminal supports
```

Inside, `F1` (or `Space ?`) lists every key, and `ZZ` quits.

Development setup, checks and the dependency policy are in [`CONTRIBUTING.md`](CONTRIBUTING.md).

## License

GPL-3.0-or-later. See [`LICENSE`](LICENSE).
