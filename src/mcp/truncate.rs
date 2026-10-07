//! Large-output handling for MCP tool responses.
//!
//! MCP clients have to render whatever a tool returns into a chat transcript.
//! Responses in the megabytes can blow out context windows and make the
//! transcript unreadable. We cap responses at a default limit and report that
//! truncation happened so the client/assistant can react (e.g., narrow the
//! range, request pagination).

use rmcp::model::{CallToolResult, ContentBlock as Content};
use serde_json::json;

/// Default maximum response size in bytes (100 KB).
///
/// Chosen as a practical balance: large enough to hold most commit-range
/// analyses, JIRA issue bodies, or Confluence page content, small enough to
/// stay well under common client context limits.
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 100 * 1024;

/// Truncation marker appended to responses that exceed the limit.
const TRUNCATION_MARKER: &str = "\n\n[output truncated]";

/// Truncates `text` to at most `limit` bytes, preserving a UTF-8 boundary.
///
/// Returns the (possibly truncated) text and a flag indicating whether
/// truncation happened. When truncated, a small marker is appended so callers
/// reading the raw text still see a clear signal; programmatic callers should
/// rely on the boolean flag instead.
///
/// A `limit` of 0 is treated as "no limit" because it is almost always a
/// configuration mistake rather than an explicit request to drop everything.
pub fn truncate_response(text: String, limit: usize) -> (String, bool) {
    if limit == 0 || text.len() <= limit {
        return (text, false);
    }

    // `floor_char_boundary` is nightly-only, so walk backwards from `limit`
    // until we land on a valid char boundary.
    let mut cutoff = limit;
    while cutoff > 0 && !text.is_char_boundary(cutoff) {
        cutoff -= 1;
    }

    let mut truncated = text;
    truncated.truncate(cutoff);
    truncated.push_str(TRUNCATION_MARKER);
    (truncated, true)
}

/// Wraps a text result in a `CallToolResult`, applying the configured response
/// cap and emitting a second `Content::text` payload carrying a JSON
/// `{"truncated": bool, "original_bytes": usize}` marker when truncation
/// happened.
///
/// Shared by every tool that can produce large output so the truncation
/// contract is consistent across the MCP surface. The cap is
/// `settings.mcp.max_response_bytes` when set, else
/// [`DEFAULT_MAX_RESPONSE_BYTES`] (issue #620).
pub(crate) fn build_truncated_result(text: String) -> CallToolResult {
    let limit = crate::utils::settings::Settings::load_mcp()
        .max_response_bytes
        .unwrap_or(DEFAULT_MAX_RESPONSE_BYTES);
    build_truncated_result_with(text, limit)
}

/// Cap-parameterised core of [`build_truncated_result`], split out so the
/// truncation contract can be unit-tested against an explicit `limit` without
/// reading `settings.json` from the caller's real home directory.
fn build_truncated_result_with(text: String, limit: usize) -> CallToolResult {
    let original_bytes = text.len();
    let (body, truncated) = truncate_response(text, limit);
    if truncated {
        let marker = json!({
            "truncated": true,
            "original_bytes": original_bytes,
            "limit_bytes": limit,
        });
        CallToolResult::success(vec![Content::text(body), Content::text(marker.to_string())])
    } else {
        CallToolResult::success(vec![Content::text(body)])
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_unchanged() {
        let (out, truncated) = truncate_response("hello".to_string(), 100);
        assert_eq!(out, "hello");
        assert!(!truncated);
    }

    #[test]
    fn exact_length_is_unchanged() {
        let (out, truncated) = truncate_response("hello".to_string(), 5);
        assert_eq!(out, "hello");
        assert!(!truncated);
    }

    #[test]
    fn over_limit_is_truncated_with_marker() {
        let input = "a".repeat(1000);
        let (out, truncated) = truncate_response(input, 100);
        assert!(truncated);
        assert!(out.len() < 1000);
        assert!(out.starts_with(&"a".repeat(100)));
        assert!(out.contains("[output truncated]"));
    }

    #[test]
    fn zero_limit_means_no_limit() {
        let input = "abc".repeat(1000);
        let original_len = input.len();
        let (out, truncated) = truncate_response(input, 0);
        assert!(!truncated);
        assert_eq!(out.len(), original_len);
    }

    #[test]
    fn utf8_boundary_preserved() {
        // Four-byte emoji; cutting in the middle would produce invalid UTF-8.
        // The input is a sequence of 🦀 (4 bytes each); cap mid-codepoint.
        let input: String = "🦀".repeat(50);
        let (out, truncated) = truncate_response(input, 10); // mid-codepoint
        assert!(truncated);
        // Must be valid UTF-8 — `String` enforces this, but the content
        // before the marker must also align to a char boundary so no partial
        // emoji are present.
        let body = out.trim_end_matches("[output truncated]");
        // Every character in the body is the crab emoji.
        for ch in body.chars() {
            assert!(ch == '🦀' || ch == '\n');
        }
    }

    #[test]
    fn default_cap_is_100kb() {
        assert_eq!(DEFAULT_MAX_RESPONSE_BYTES, 102_400);
    }

    #[test]
    fn empty_string_is_not_truncated() {
        let (out, truncated) = truncate_response(String::new(), 100);
        assert_eq!(out, "");
        assert!(!truncated);
    }

    #[test]
    fn build_truncated_result_leaves_small_output_alone() {
        let result = build_truncated_result("hello".to_string());
        assert_eq!(result.content.len(), 1);
    }

    #[test]
    fn build_truncated_result_appends_marker_when_over_cap() {
        // Use the cap-parameterised core so the assertion is independent of any
        // `settings.json` in the test runner's real home directory.
        let big = "x".repeat(DEFAULT_MAX_RESPONSE_BYTES + 1024);
        let result = build_truncated_result_with(big, DEFAULT_MAX_RESPONSE_BYTES);
        assert_eq!(result.content.len(), 2, "expected body + truncation marker");
        let marker_raw = result.content[1]
            .as_text()
            .expect("second payload should be text")
            .text
            .clone();
        let parsed: serde_json::Value = serde_json::from_str(&marker_raw).expect("marker is JSON");
        assert_eq!(parsed["truncated"], serde_json::Value::Bool(true));
        let original = parsed["original_bytes"].as_u64().unwrap();
        let limit = parsed["limit_bytes"].as_u64().unwrap();
        assert!(original > limit);
        assert_eq!(limit, DEFAULT_MAX_RESPONSE_BYTES as u64);
    }

    #[test]
    fn build_truncated_result_with_honours_custom_cap() {
        // A settings-provided `max_response_bytes` (issue #620) truncates at the
        // configured limit and reports it in the marker.
        let custom_limit = 32usize;
        let result = build_truncated_result_with("y".repeat(1024), custom_limit);
        assert_eq!(result.content.len(), 2, "expected body + truncation marker");
        let marker_raw = result.content[1]
            .as_text()
            .expect("second payload should be text")
            .text
            .clone();
        let parsed: serde_json::Value = serde_json::from_str(&marker_raw).expect("marker is JSON");
        assert_eq!(parsed["limit_bytes"].as_u64().unwrap(), custom_limit as u64);
        assert_eq!(parsed["original_bytes"].as_u64().unwrap(), 1024);
    }

    #[test]
    fn build_truncated_result_with_zero_cap_disables_truncation() {
        // `0` means "no limit" (matching `truncate_response`), so a large body
        // passes through untouched with no marker.
        let result = build_truncated_result_with("z".repeat(DEFAULT_MAX_RESPONSE_BYTES + 1), 0);
        assert_eq!(result.content.len(), 1, "no truncation marker expected");
    }
}
