// SPDX-License-Identifier: GPL-3.0-or-later
//! Background services for STEDE.md: file tree and watcher (polling on
//! `/mnt/*` paths in WSL), the linter hub (LSP and CLI adapters), and
//! external commands. Runs off the draw loop and reports back on a channel.
//! No UI code here.
