//! Placeholder entry point for `gwi`, the Google Workspace Interface.
//!
//! The Gmail and Drive command trees are being extracted from omni-dev
//! (rust-works/omni-dev#2203); until they land this binary only reports that.

fn main() {
    println!(
        "gwi {}: placeholder release. The Google Workspace Interface is still being \
         extracted from omni-dev; for now use `omni-dev gmail` and `omni-dev drive`. \
         See https://github.com/rust-works/gwi.",
        env!("CARGO_PKG_VERSION")
    );
}
