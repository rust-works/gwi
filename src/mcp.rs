//! MCP (Model Context Protocol) server implementation.
//!
//! Exposes gwi's Gmail and Drive (with Docs and Sheets) operations to AI assistants
//! over MCP, from the separate `gwi-mcp` binary (ADR-0001). The module layout and
//! the tool handlers are ported from omni-dev's `mcp` module
//! (rust-works/omni-dev#2203); the tools keep their names so an existing MCP client
//! configuration only needs to point at the new server command.

pub mod content_input;
pub mod drive_docs_tools;
pub mod drive_sheets_tools;
pub mod drive_tools;
pub mod drive_write_tools;
pub mod error;
pub mod gmail_tools;
pub mod output_file;
pub mod path_policy;
pub mod runtime;
pub mod server;
pub mod truncate;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod path_policy_guard;

pub use error::tool_error;
pub use runtime::{log_startup_event, serve_with, try_init_tracing, write_error_chain};
pub use server::GwiServer;
pub use truncate::{truncate_response, DEFAULT_MAX_RESPONSE_BYTES};
