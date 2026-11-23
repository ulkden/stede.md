// SPDX-License-Identifier: GPL-3.0-or-later
//! `stede`: the STEDE.md terminal front end.

fn main() {
    println!(
        "{} {} (phase 0 scaffold)",
        stede_core::APP_NAME,
        env!("CARGO_PKG_VERSION")
    );
}
