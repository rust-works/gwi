//! Shared helper for MCP read-style tools that optionally write to disk.
//!
//! Large pages returned inline by MCP tools can blow past the assistant's
//! context window. When a caller supplies an `output_file`, the rendered
//! content is written to disk and the tool returns a short YAML summary
//! pointing at the file so the assistant can page through it via its
//! filesystem read tool. See issue #631 for the motivating use case.

use anyhow::{Context, Result};
use serde::Serialize;

use super::path_policy::PathPolicy;

/// Summary returned to the assistant when a read tool wrote its output to a
/// file rather than returning the content inline.
#[derive(Debug, Serialize)]
pub struct WriteFileSummary {
    /// Path the content was written to (as supplied by the caller).
    pub path: String,
    /// Number of bytes written.
    pub bytes: usize,
    /// Output format identifier (e.g. `"jfm"`, `"adf"`).
    pub format: String,
}

/// Writes `content` to `path`, if `policy` allows it, and returns a YAML-encoded
/// [`WriteFileSummary`].
pub(crate) fn write_to_file_yaml(
    policy: &PathPolicy,
    path: &str,
    content: &str,
    format: &str,
) -> Result<String> {
    policy.write(path, content.as_bytes())?;
    let summary = WriteFileSummary {
        path: path.to_string(),
        bytes: content.len(),
        format: format.to_string(),
    };
    serde_yaml::to_string(&summary).context("Failed to serialize write summary as YAML")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A policy that allows the system temp directory, where `tempfile` puts its
    /// directories, and nothing under the user's real home.
    fn policy() -> PathPolicy {
        PathPolicy::for_tests()
    }

    #[test]
    fn write_to_file_yaml_writes_content_and_summarises() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("out.md");
        let path_str = path.to_str().unwrap();

        let yaml = write_to_file_yaml(&policy(), path_str, "hello world", "jfm").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world");
        assert!(yaml.contains(&format!("path: {path_str}")));
        assert!(yaml.contains("bytes: 11"));
        assert!(yaml.contains("format: jfm"));
    }

    #[test]
    fn write_to_file_yaml_overwrites_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("out.md");
        std::fs::write(&path, "old").unwrap();

        write_to_file_yaml(&policy(), path.to_str().unwrap(), "new", "jfm").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    }

    #[test]
    fn write_to_file_yaml_reports_byte_count_for_unicode() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("out.md");

        // "héllo" = 6 bytes (é is 2 bytes in UTF-8).
        let yaml = write_to_file_yaml(&policy(), path.to_str().unwrap(), "héllo", "jfm").unwrap();

        assert!(yaml.contains("bytes: 6"));
    }

    #[test]
    fn write_to_file_yaml_errors_on_invalid_path() {
        let err = write_to_file_yaml(&policy(), "/nonexistent_dir_zxq/file.txt", "data", "jfm")
            .unwrap_err();
        assert!(err.to_string().contains("Failed to resolve"));
    }

    #[test]
    fn write_to_file_yaml_uses_supplied_format_label() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("out.json");
        let yaml = write_to_file_yaml(&policy(), path.to_str().unwrap(), "{}", "adf").unwrap();
        assert!(yaml.contains("format: adf"));
    }

    #[test]
    fn write_to_file_yaml_refuses_a_path_outside_the_policy_without_writing() {
        let allowed = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let policy = PathPolicy::allowing_only(allowed.path());
        let target = outside.path().join("out.md");

        let err = write_to_file_yaml(&policy, target.to_str().unwrap(), "x", "jfm").unwrap_err();

        assert!(err.to_string().contains("outside the allowed"), "{err}");
        assert!(!target.exists());
    }
}
