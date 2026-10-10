//! Integration tests for the MCP server.
//!
//! Some tests run `GwiServer` on one end of an in-memory duplex transport with a
//! generic rmcp client on the other, and exercise the protocol end to end. Other tests
//! spawn the real `gwi-mcp` binary and talk to it over stdio, which is how an
//! assistant actually uses it.
//!
//! None of them reaches the network or a browser. The spawned binary runs with a
//! cleared environment and an empty `HOME`, and only calls tools that fail or
//! answer before any API request is made. Instrumented runs preserve only LLVM's
//! explicit profile destination from the parent environment.

#![cfg(feature = "mcp")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[cfg(unix)]
use std::{process::Stdio, time::Duration};

#[cfg(unix)]
use anyhow::Context;
use anyhow::Result;
use rmcp::{
    model::{CallToolRequestParams, CallToolResult, ContentBlock},
    service::ServiceExt,
    ClientHandler, RoleClient,
};

use gwi::mcp::GwiServer;

const GMAIL_TOOLS: [&str; 8] = [
    "gmail_account_list",
    "gmail_auth_status",
    "gmail_draft_list",
    "gmail_draft_show",
    "gmail_label_list",
    "gmail_message_read",
    "gmail_search",
    "gmail_thread_read",
];

const DRIVE_TOOLS: [&str; 15] = [
    "drive_account_list",
    "drive_auth_status",
    "drive_dedupe",
    "drive_docs_append",
    "drive_docs_info",
    "drive_docs_read",
    "drive_docs_replace",
    "drive_file_read",
    "drive_lease_acquire",
    "drive_search",
    "drive_sheets_append",
    "drive_sheets_clear",
    "drive_sheets_info",
    "drive_sheets_read",
    "drive_sheets_write",
];

/// Every tool `gwi-mcp` serves, sorted.
fn all_tools() -> Vec<&'static str> {
    let mut names: Vec<_> = GMAIL_TOOLS.iter().chain(&DRIVE_TOOLS).copied().collect();
    names.sort_unstable();
    names
}

struct TestClient;

impl ClientHandler for TestClient {}

type Client = rmcp::service::RunningService<RoleClient, TestClient>;

/// Starts `GwiServer` on an in-memory transport and connects a client to it.
async fn spawn_server() -> (Client, tokio::task::JoinHandle<Result<()>>) {
    let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
    let server_handle = tokio::spawn(async move {
        let service = GwiServer::new().serve(server_transport).await?;
        service.waiting().await?;
        Ok(())
    });
    let client = TestClient.serve(client_transport).await.unwrap();
    (client, server_handle)
}

fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match c {
            ContentBlock::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn list_tools_advertises_exactly_the_gmail_and_drive_tools() -> Result<()> {
    let (client, server_handle) = spawn_server().await;

    let tools = client.list_tools(Option::default()).await?;

    let mut names: Vec<_> = tools.tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(names, all_tools());
    for tool in &tools.tools {
        let description = tool.description.as_deref().unwrap_or_default();
        assert!(!description.is_empty(), "{} has no description", tool.name);
        if let Some(properties) = tool.input_schema.get("properties") {
            let properties = properties
                .as_object()
                .unwrap_or_else(|| panic!("{}: schema properties must be an object", tool.name));
            for (field, schema) in properties {
                let description = schema
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                assert!(
                    !description.trim().is_empty(),
                    "{}: parameter `{field}` has no non-empty description",
                    tool.name
                );
            }
        }
    }
    client.cancel().await?;
    let _ = server_handle.await;
    Ok(())
}

/// Parameter descriptions are plain text; explicit tool Markdown is intentional.
#[tokio::test]
async fn schema_descriptions_are_plain_text_and_keep_examples() -> Result<()> {
    let (client, server_handle) = spawn_server().await;
    let tools = client.list_tools(Option::default()).await?;
    let mut parameters = 0;
    for tool in &tools.tools {
        if let Some(properties) = tool
            .input_schema
            .get("properties")
            .and_then(|v| v.as_object())
        {
            for (field, schema) in properties {
                let text = schema["description"].as_str().unwrap();
                assert!(
                    !text.contains('`') && !text.contains("]("),
                    "Markdown in {} parameter {field}: {text}",
                    tool.name
                );
                parameters += 1;
            }
        }
    }
    assert_eq!(
        parameters, 90,
        "All advertised top-level parameters audited"
    );
    let description = |tool_name: &str, field: &str| {
        let tool = tools.tools.iter().find(|t| t.name == tool_name).unwrap();
        tool.input_schema["properties"][field]["description"]
            .as_str()
            .unwrap()
    };
    assert!(description("gmail_search", "query").contains("label:finance after:2026/01/01"));
    let enrich = description("gmail_search", "enrich");
    assert!(
        enrich.contains("messages.get") && enrich.contains("false") && enrich.contains("20 units")
    );
    assert!(description("drive_sheets_write", "values")
        .contains(r#"[["name", "score"], ["Ada", "42"]]"#));
    assert!(description("drive_docs_append", "text_path").contains("mcp.allowed_paths"));
    assert!(description("drive_docs_read", "tab").contains("tabs[].tab_id"));
    for tool in &tools.tools {
        assert!(
            tool.description.as_deref().unwrap().contains('`'),
            "{} lost its intentional tool-description formatting",
            tool.name
        );
    }
    client.cancel().await?;
    let _ = server_handle.await;
    Ok(())
}

/// Rustdoc code delimiters must not leak into URL identifier schema hints.
#[tokio::test]
async fn identifier_schema_hints_have_no_markdown() -> Result<()> {
    let (client, server_handle) = spawn_server().await;
    let tools = client.list_tools(Option::default()).await?;
    let mut hints = 0;
    for tool in &tools.tools {
        let Some(properties) = tool
            .input_schema
            .get("properties")
            .and_then(|v| v.as_object())
        else {
            continue;
        };
        for (field, schema) in properties {
            let description = schema["description"].as_str().unwrap();
            if description.contains("/d/<ID>/") {
                assert!(
                    !description.contains('`'),
                    "markdown in {} parameter {field}: {description}",
                    tool.name
                );
                hints += 1;
            }
        }
    }
    assert_eq!(
        hints, 9,
        "Docs/Sheets read and write identifier descriptions"
    );
    client.cancel().await?;
    let _ = server_handle.await;
    Ok(())
}

#[tokio::test]
async fn every_tool_takes_an_optional_account() -> Result<()> {
    let (client, server_handle) = spawn_server().await;
    let tools = client.list_tools(Option::default()).await?;

    for tool in &tools.tools {
        let name = tool.name.as_ref();
        let props = tool
            .input_schema
            .get("properties")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let required = tool
            .input_schema
            .get("required")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        // The two account lists list the accounts, so they are the tools that take none.
        if name == "gmail_account_list" || name == "drive_account_list" {
            assert!(!props.contains_key("account"), "{name}");
            continue;
        }
        assert!(
            props.contains_key("account"),
            "{name}: no `account` parameter"
        );
        assert!(
            !required.contains(&serde_json::json!("account")),
            "{name}: `account` must be optional"
        );
    }
    client.cancel().await?;
    let _ = server_handle.await;
    Ok(())
}

/// Both draft tools are advertised with a description and a described schema for
/// every parameter, including the required `draft_id` (omni-dev#1957).
#[tokio::test]
async fn list_tools_includes_gmail_draft_tools() -> Result<()> {
    let (client, server_handle) = spawn_server().await;
    let tools = client.list_tools(Option::default()).await?;

    for (name, params) in [
        ("gmail_draft_list", &["query", "limit", "account"][..]),
        (
            "gmail_draft_show",
            &["draft_id", "format", "output_file", "account"][..],
        ),
    ] {
        let tool = tools
            .tools
            .iter()
            .find(|t| t.name.as_ref() == name)
            .unwrap_or_else(|| panic!("{name} not advertised"));
        let description = tool.description.as_deref().unwrap_or_default();
        assert!(
            description.contains("draft id"),
            "{name}: description should talk about draft ids: {description}"
        );
        let props = tool
            .input_schema
            .get("properties")
            .and_then(|v| v.as_object())
            .unwrap_or_else(|| panic!("{name}: no properties in schema"));
        for param in params {
            assert!(props.contains_key(*param), "{name}: missing param {param}");
        }
    }

    let show = tools
        .tools
        .iter()
        .find(|t| t.name.as_ref() == "gmail_draft_show")
        .unwrap();
    let required = show
        .input_schema
        .get("required")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert_eq!(required, vec![serde_json::json!("draft_id")]);

    client.cancel().await?;
    let _ = server_handle.await;
    Ok(())
}

// Requires HOME-based settings isolation; Windows uses the Known Folder API.
#[cfg(unix)]
#[tokio::test]
async fn an_unknown_tool_is_a_protocol_error_not_a_panic() -> Result<()> {
    // Unknown tools are request-logged. Give the real server its own fixture
    // HOME so this integration test cannot append to the host request log.
    let home = tempfile::tempdir()?;
    let (client, child) = spawn_binary(home.path()).await?;

    let result = client
        .call_tool(CallToolRequestParams::new("gmail_send"))
        .await;

    let error = result.expect_err("send is deliberately not a tool");
    assert!(error.to_string().contains("tool not found"), "{error}");
    shutdown_binary(client, child).await?;
    Ok(())
}

/// Schemas and dispatch reject policy overrides before credentials or consent
/// (omni-dev's `drive_write_tools_round_trip_and_reject_policy_parameters`).
// Requires HOME-based settings/state isolation; Windows uses the Known Folder API.
#[cfg(unix)]
#[tokio::test]
async fn drive_write_tools_round_trip_and_reject_policy_parameters() -> Result<()> {
    // Policy rejections write an audit record. Confine it to this fixture's HOME
    // through the real server process, rather than the integration binary's HOME.
    let home = tempfile::tempdir()?;
    let (client, child) = spawn_binary(home.path()).await?;
    let tools = client.list_tools(Option::default()).await?;
    for (name, arguments) in [
        (
            "drive_docs_replace",
            serde_json::json!({"document_id":"target","search":"a","replace":"b"}),
        ),
        (
            "drive_docs_append",
            serde_json::json!({"document_id":"target","text":"a"}),
        ),
        (
            "drive_sheets_write",
            serde_json::json!({"spreadsheet_id":"target","values":[["a"]]}),
        ),
        (
            "drive_sheets_append",
            serde_json::json!({"spreadsheet_id":"target","values":[["a"]]}),
        ),
        (
            "drive_sheets_clear",
            serde_json::json!({"spreadsheet_id":"target","range":"A1"}),
        ),
        (
            "drive_lease_acquire",
            serde_json::json!({"file_id":"target"}),
        ),
    ] {
        let tool = tools.tools.iter().find(|t| t.name == name).unwrap();
        let properties = tool
            .input_schema
            .get("properties")
            .unwrap()
            .as_object()
            .unwrap();
        assert!(properties.contains_key("account"));
        for forbidden in [
            "allow_headless",
            "biometrics_only",
            "ledger_path",
            "supersedes",
            "rules",
        ] {
            assert!(
                !properties.contains_key(forbidden),
                "{name} exposes {forbidden}"
            );
        }
        let mut arguments = arguments.as_object().unwrap().clone();
        arguments.insert("allow_headless".into(), serde_json::json!(true));
        let outcome = client
            .call_tool(CallToolRequestParams::new(name).with_arguments(arguments))
            .await;
        let message = match outcome {
            Err(err) => err.to_string(),
            Ok(result) => {
                assert_eq!(result.is_error, Some(true));
                text_of(&result)
            }
        };
        assert!(
            message.contains("unknown field") && message.contains("allow_headless"),
            "{name}: {message}"
        );
    }
    shutdown_binary(client, child).await?;
    Ok(())
}

/// Spawns the real `gwi-mcp` binary hermetically in an empty `HOME`.
// Windows' Known Folder API ignores HOME; this subprocess fixture cannot
// isolate its settings from the user's real profile there.
#[cfg(unix)]
async fn spawn_binary(home: &std::path::Path) -> Result<(Client, tokio::process::Child)> {
    let profile = std::env::var_os("LLVM_PROFILE_FILE");
    spawn_binary_with_profile(home, profile.as_deref()).await
}

// Keep coverage output explicit without inheriting credentials or API endpoints.
#[cfg(unix)]
async fn spawn_binary_with_profile(
    home: &std::path::Path,
    profile: Option<&std::ffi::OsStr>,
) -> Result<(Client, tokio::process::Child)> {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_gwi-mcp"));
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home)
        .env("GWI_LOG_DISABLE", "1")
        .current_dir(home)
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(profile) = profile {
        // LLVM's process/module placeholders keep concurrent children distinct.
        command.env("LLVM_PROFILE_FILE", profile);
    }
    let mut child = command.spawn()?;
    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let client =
        tokio::time::timeout(Duration::from_secs(20), TestClient.serve((stdout, stdin))).await??;
    Ok((client, child))
}

/// The real binary serves the same 23 tools over stdio, answers the one tool
/// that needs no credentials, and reports the others as tool errors rather than
/// protocol failures.
// Requires HOME-based settings isolation; Windows uses the Known Folder API.
#[cfg(unix)]
#[tokio::test]
async fn the_binary_serves_the_tools_over_stdio() -> Result<()> {
    let home = tempfile::tempdir()?;
    let (client, child) = spawn_binary(home.path()).await?;
    let call = |name: &'static str, args: serde_json::Value| {
        let client = &client;
        async move {
            tokio::time::timeout(
                Duration::from_secs(20),
                client.call_tool(
                    CallToolRequestParams::new(name)
                        .with_arguments(args.as_object().unwrap().clone()),
                ),
            )
            .await
        }
    };

    let info = client.peer_info().expect("server info");
    assert_eq!(
        info.server_info.as_ref().map(|i| i.name.as_str()),
        Some("gwi-mcp")
    );

    let tools = client.list_tools(Option::default()).await?;
    assert_eq!(tools.tools.len(), all_tools().len());

    // No accounts are configured: a successful, empty answer, not an error.
    let accounts = call("gmail_account_list", serde_json::json!({})).await??;
    assert_ne!(accounts.is_error, Some(true), "{}", text_of(&accounts));

    // Likewise for Drive: the account list and the auth status answer locally.
    for name in ["drive_account_list", "drive_auth_status"] {
        let answer = call(name, serde_json::json!({})).await??;
        assert_ne!(answer.is_error, Some(true), "{name}: {}", text_of(&answer));
    }

    // Everything that would call the API fails first on missing credentials.
    for (name, args) in [
        ("gmail_search", serde_json::json!({"query": "is:unread"})),
        ("gmail_label_list", serde_json::json!({})),
        ("gmail_draft_list", serde_json::json!({})),
        ("gmail_draft_show", serde_json::json!({"draft_id": "r1"})),
        (
            "drive_search",
            serde_json::json!({"query": "name contains 'x'"}),
        ),
        (
            "drive_dedupe",
            serde_json::json!({"query": "name contains 'x'"}),
        ),
        ("drive_file_read", serde_json::json!({"file_id": "f1"})),
        ("drive_docs_info", serde_json::json!({"document_id": "d1"})),
        ("drive_docs_read", serde_json::json!({"document_id": "d1"})),
        (
            "drive_sheets_info",
            serde_json::json!({"spreadsheet_id": "s1"}),
        ),
        (
            "drive_sheets_read",
            serde_json::json!({"spreadsheet_id": "s1", "range": "A1"}),
        ),
    ] {
        let failed = match call(name, args).await? {
            Ok(result) => {
                result.is_error.unwrap_or(false) && text_of(&result).contains("not configured")
            }
            Err(err) => err.to_string().contains("not configured"),
        };
        assert!(failed, "{name} should fail with a credentials error");
    }

    shutdown_binary(client, child).await?;
    Ok(())
}

/// Close stdin and wait for a normal exit so LLVM can flush the child's profile.
#[cfg(unix)]
async fn shutdown_binary(client: Client, mut child: tokio::process::Child) -> Result<()> {
    let status = tokio::time::timeout(Duration::from_secs(20), async {
        client.cancel().await?;
        Ok::<_, anyhow::Error>(child.wait().await?)
    })
    .await
    .context("gwi-mcp did not shut down within 20 seconds")??;
    assert!(status.success(), "gwi-mcp exited with {status}");
    Ok(())
}

/// The instrumented server must flush into the supplied fixture before cleanup.
// Requires an instrumented binary and HOME-based isolation; Windows ignores HOME.
#[cfg(unix)]
#[tokio::test]
async fn the_binary_flushes_its_coverage_profile_on_shutdown() -> Result<()> {
    if std::env::var_os("CARGO_LLVM_COV").is_none() {
        return Ok(());
    }
    let home = tempfile::tempdir()?;
    let profiles = tempfile::tempdir()?;
    let profile = profiles.path().join("child.profraw");
    let (client, child) = spawn_binary_with_profile(home.path(), Some(profile.as_os_str())).await?;
    let result = client
        .call_tool(CallToolRequestParams::new("gmail_send"))
        .await;
    assert!(result.is_err(), "send is deliberately not a tool");
    shutdown_binary(client, child).await?;
    assert!(std::fs::metadata(profile)?.len() > 0, "empty LLVM profile");
    Ok(())
}
