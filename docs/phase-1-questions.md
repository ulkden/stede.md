# Phase 1: questions for the owner

Collected during the autonomous phase 1 run. Each entry gives the question, the options, and what was done meanwhile.

1. **MSRV policy.** Dependencies keep raising the minimum Rust version: ratatui 0.30.2 needs 1.88, and comrak 0.59 needs 1.89. Meanwhile each bump has followed what the dependency needs. Should the MSRV track "pinned toolchain minus N releases" instead, or be dropped for a binary-only app?
2. **comrak 1.x.** 1.0.0 was released 2026-11-17, so it's eligible from 2026-12-01. Step 3 starts on 0.59.0, and Dependabot will propose the major bump. Is moving to 1.x once it's eligible fine?
