//! Binary entry point for the gwi MCP server.
//!
//! Speaks the Model Context Protocol over stdio so AI assistants can invoke gwi's
//! Gmail and Drive operations as MCP tools. All non-trivial logic lives in `gwi::mcp::runtime`
//! so it can be exercised by library tests; this binary is intentionally a thin shim.

use std::process;

use gwi::mcp;
use gwi::utils::settings::Settings;
use rmcp::transport::stdio;

#[tokio::main]
async fn main() {
    // MCP defaults from `settings.json`: the log level seeds the tracing filter's
    // fallback, but `RUST_LOG` still wins when set.
    let mcp_settings = Settings::load_mcp();
    let _ = mcp::try_init_tracing(mcp_settings.log_level.as_deref());
    mcp::log_startup_event();
    if let Err(e) = mcp::serve_with(stdio()).await {
        let mut stderr = std::io::stderr().lock();
        let _ = mcp::write_error_chain(&mut stderr, &e);
        process::exit(1);
    }
}
