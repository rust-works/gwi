//! MCP server setup: tool router composition and protocol capabilities.

use rmcp::{
    handler::server::router::tool::ToolRouter,
    model::{CallToolResponse, Implementation, ProtocolVersion, ServerCapabilities, ServerConfig},
    service::RequestContext,
    tool_handler, ErrorData as McpError, RoleServer, ServerHandler,
};

use crate::request_log;

/// The gwi MCP server.
///
/// All tool handlers are defined on this struct via `#[tool_router]` in submodules
/// under `src/mcp/`. Routers are combined in [`Self::new`].
#[derive(Clone)]
pub struct GwiServer {
    /// Combined tool router.
    pub tool_router: ToolRouter<Self>,
}

impl Default for GwiServer {
    fn default() -> Self {
        Self::new()
    }
}

impl GwiServer {
    /// Constructs a new server with all tool routers combined.
    pub fn new() -> Self {
        Self {
            tool_router: Self::gmail_tool_router(),
        }
    }
}

// `ServerHandler`'s methods are `async fn` in the trait (rmcp, an external
// crate), so an impl can't drop `async` even where a given method never
// awaits — that also covers the `#[tool_handler]` macro's own generated
// method, which this lint can't see into.
#[allow(clippy::unused_async_trait_impl)]
#[tool_handler(router = self.tool_router)]
impl ServerHandler for GwiServer {
    /// Routes an MCP tool call, scoping a task-local request-log context
    /// (`source = mcp`, the tool name) around the dispatch so any HTTP the tool
    /// issues correlates to it, and appending one invocation record per call.
    ///
    /// Mirrors the dispatch the `#[tool_handler]` macro would otherwise
    /// generate; defining it here makes the macro skip its own `call_tool`.
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let tool = request.name.to_string();
        let ctx = request_log::RequestLogContext::mcp(tool.clone());
        request_log::CTX
            .scope(ctx, async move {
                let started = std::time::Instant::now();
                let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
                let result = self.tool_router.call(tcc).await;
                let (exit_code, error) = match &result {
                    Ok(_) => (0, None),
                    Err(e) => (1, Some(e.to_string())),
                };
                request_log::record_invocation(request_log::InvocationOutcome {
                    command: vec![tool],
                    command_line: Vec::new(),
                    exit_code,
                    error,
                    duration: started.elapsed(),
                });
                result
            })
            .await
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("gwi-mcp", env!("CARGO_PKG_VERSION")))
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(
                "gwi MCP server. Provides read-only tools for Gmail: searching and reading \
                 messages and threads, listing labels, drafts and configured accounts, and \
                 checking authentication status. Every tool takes an optional `account` to \
                 select a named account from ~/.gwi/settings.json. Sign in first with \
                 `gwi gmail auth login`; there is no MCP tool for the interactive login.",
            )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const GMAIL_TOOLS: [&str; 8] = [
        "gmail_auth_status",
        "gmail_search",
        "gmail_message_read",
        "gmail_thread_read",
        "gmail_label_list",
        "gmail_account_list",
        "gmail_draft_list",
        "gmail_draft_show",
    ];

    #[test]
    fn server_info_advertises_only_the_tools_capability() {
        let info = GwiServer::new().get_info();
        assert!(info.capabilities.tools.is_some());
        assert!(
            info.capabilities.resources.is_none(),
            "gwi serves no resources"
        );
        assert_eq!(info.server_info.name, "gwi-mcp");
        assert_eq!(info.server_info.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn tool_router_registers_all_gmail_tools() {
        let server = GwiServer::new();
        for name in GMAIL_TOOLS {
            assert!(server.tool_router.has_route(name), "missing route: {name}");
        }
    }

    #[test]
    fn tool_router_lists_exactly_the_gmail_tools() {
        let server = GwiServer::new();
        let mut names: Vec<String> = server
            .tool_router
            .list_all()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        names.sort();
        let mut expected: Vec<String> = GMAIL_TOOLS.iter().map(|s| (*s).to_string()).collect();
        expected.sort();
        assert_eq!(names, expected);
    }

    #[test]
    fn every_tool_is_read_only_and_none_can_send_or_delete() {
        // ADR-0001 / omni-dev#1920: send and delete are not MCP tools.
        for tool in GwiServer::new().tool_router.list_all() {
            let name = tool.name.to_string();
            assert!(
                !name.contains("send") && !name.contains("delete") && !name.contains("trash"),
                "unexpected mutating tool: {name}"
            );
        }
    }
}
