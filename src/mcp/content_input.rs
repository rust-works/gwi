//! Shared helper for MCP write tools that accept a body either inline or as a
//! filesystem path.
//!
//! AI callers pay an O(size) generation cost to emit a large body inline
//! through a tool call — the model has to write every byte through its output
//! stream. When the body is already on disk, a `*_path` parameter lets the
//! server read it directly, sidestepping that cost. This is the write-side
//! mirror of the read-side `output_file` parameter (see [`super::output_file`]).
//!
//! Only the bounded variant the Drive writes use is ported from omni-dev; its
//! unbounded `resolve_content_input` / `require_content_input` siblings serve
//! Atlassian tools gwi does not have.

use anyhow::{Context, Result};

/// Resolves mandatory inline/file UTF-8 content with a byte cap.
///
/// Used by Drive writes. File reads are bounded on the opened handle, so a
/// file growing after its size check cannot cause an unbounded allocation.
/// Special files are refused; on Unix nonblocking open also prevents a FIFO
/// replacement from waiting for a writer before its type can be checked.
pub(crate) fn require_bounded_content_input(
    inline: Option<&str>,
    path: Option<&str>,
    field: &str,
    cap: u64,
) -> Result<String> {
    use std::io::Read as _;

    match (inline, path) {
        (Some(_), Some(_)) => {
            anyhow::bail!("Provide either `{field}` or `{field}_path`, not both.")
        }
        (Some(text), None) => {
            anyhow::ensure!(
                text.len() as u64 <= cap,
                "`{field}` exceeds the {cap} byte cap"
            );
            Ok(text.to_owned())
        }
        (None, Some(path)) => {
            anyhow::ensure!(
                path != "-",
                "`{field}_path` must be a file; stdin is unsupported"
            );
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.custom_flags(nix::libc::O_NONBLOCK);
            }
            let file = options
                .open(path)
                .with_context(|| format!("Failed to open `{field}_path` file {path}"))?;
            let metadata = file
                .metadata()
                .with_context(|| format!("Failed to stat `{field}_path` file {path}"))?;
            anyhow::ensure!(
                metadata.is_file(),
                "`{field}_path` must be a regular UTF-8 file"
            );
            anyhow::ensure!(
                metadata.len() <= cap,
                "`{field}_path` exceeds the {cap} byte cap"
            );
            let mut bytes = Vec::new();
            file.take(cap.saturating_add(1))
                .read_to_end(&mut bytes)
                .with_context(|| format!("Failed to read `{field}_path` file {path}"))?;
            anyhow::ensure!(
                bytes.len() as u64 <= cap,
                "`{field}_path` exceeds the {cap} byte cap"
            );
            String::from_utf8(bytes)
                .with_context(|| format!("`{field}_path` file {path} is not valid UTF-8"))
        }
        (None, None) => anyhow::bail!("Provide either `{field}` or `{field}_path`."),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn bounded_input_accepts_the_cap_and_rejects_larger_or_invalid_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text.txt");
        std::fs::write(&path, "abcd").unwrap();
        assert_eq!(
            require_bounded_content_input(None, Some(path.to_str().unwrap()), "text", 4).unwrap(),
            "abcd"
        );
        assert_eq!(
            require_bounded_content_input(Some("abcd"), None, "text", 4).unwrap(),
            "abcd"
        );
        assert!(
            require_bounded_content_input(Some("abcde"), None, "text", 4)
                .unwrap_err()
                .to_string()
                .contains("cap")
        );
        assert!(
            require_bounded_content_input(None, Some(path.to_str().unwrap()), "text", 3)
                .unwrap_err()
                .to_string()
                .contains("cap")
        );
        std::fs::write(&path, [0xff]).unwrap();
        assert!(
            require_bounded_content_input(None, Some(path.to_str().unwrap()), "text", 4)
                .unwrap_err()
                .to_string()
                .contains("UTF-8")
        );
        assert!(
            require_bounded_content_input(Some("x"), Some("missing"), "text", 4)
                .unwrap_err()
                .to_string()
                .contains("not both")
        );
    }

    #[cfg(unix)]
    #[test]
    fn bounded_input_refuses_devices_and_fifos_without_reading_them() {
        assert!(
            require_bounded_content_input(None, Some("/dev/zero"), "text", 4)
                .unwrap_err()
                .to_string()
                .contains("regular")
        );
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe");
        nix::unistd::mkfifo(
            &fifo,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .unwrap();
        assert!(
            require_bounded_content_input(None, Some(fifo.to_str().unwrap()), "text", 4)
                .unwrap_err()
                .to_string()
                .contains("regular")
        );
    }
}
