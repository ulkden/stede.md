# GFM spec (test data)

`gfm-spec.txt` is the GitHub Flavored Markdown spec, version 0.29, copied unchanged from [github/cmark-gfm](https://github.com/github/cmark-gfm) at commit `7fe8904ee31fa24b772ae565c4908b1394850de6` (`test/spec.txt`).

It is licensed under [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/), as its own front matter states. Creative Commons lists GPLv3 as compatible with BY-SA 4.0 (one way, BY-SA into GPLv3). It is test data only: `tests/gfm_spec.rs` reads it, and it is never built into the `stede` binary.

To update it, replace the file with a newer upstream copy, update the commit above, and rerun `cargo test -p stede-markdown`.
