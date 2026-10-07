//! # gwi
//!
//! Google Workspace Interface: Gmail, Drive, Docs, Sheets and Slides from the
//! command line and as MCP tools.
//!
//! This crate is being assembled from code extracted from omni-dev
//! (rust-works/omni-dev#2203). The shared infrastructure (secrets, settings,
//! request log, filesystem helpers) is forked from omni-dev at a recorded baseline;
//! the product modules follow.

// Temporary: the forked helpers are unused until the Gmail modules are wired in, in the
// next commit, which removes this.
#![allow(dead_code)]
#![warn(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

pub mod cli;
pub mod request_log;
pub mod utils;

#[cfg(test)]
pub(crate) mod test_support;

/// The current version of gwi.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
