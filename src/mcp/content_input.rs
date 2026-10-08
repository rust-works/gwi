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

use super::path_policy::PathPolicy;

/// Opens `resolved` for reading and checks that it is a regular file.
///
/// The open is nonblocking on Unix so a FIFO swapped in for the file cannot wait for a
/// writer before its type is known. Once the regular-file check passes the flag is
/// cleared again: a leftover `O_NONBLOCK` on some FUSE and network mounts turns a read
/// into a confusing `EAGAIN` error.
fn open_regular_file(
    resolved: &std::path::Path,
    field: &str,
    shown: &str,
) -> Result<(std::fs::File, std::fs::Metadata)> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(nix::libc::O_NONBLOCK);
    }
    let file = options
        .open(resolved)
        .with_context(|| format!("Failed to open `{field}_path` file {shown}"))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("Failed to stat `{field}_path` file {shown}"))?;
    anyhow::ensure!(
        metadata.is_file(),
        "`{field}_path` must be a regular UTF-8 file"
    );
    #[cfg(unix)]
    clear_nonblocking(&file)
        .with_context(|| format!("Failed to clear O_NONBLOCK on `{field}_path` file {shown}"))?;
    Ok((file, metadata))
}

/// Clears `O_NONBLOCK` on an open file description.
#[cfg(unix)]
fn clear_nonblocking(file: &std::fs::File) -> nix::Result<()> {
    use nix::fcntl::{fcntl, FcntlArg, OFlag};

    let flags = OFlag::from_bits_retain(fcntl(file, FcntlArg::F_GETFL)?);
    fcntl(file, FcntlArg::F_SETFL(flags & !OFlag::O_NONBLOCK)).map(|_| ())
}

/// Resolves mandatory inline/file UTF-8 content with a byte cap.
///
/// Used by Drive writes. A `*_path` source is checked against `policy` before it is
/// opened (#45), so a path outside the operator's allowed directories, or inside a
/// credential location, is refused without being read. File reads are bounded on the
/// opened handle, so a file growing after its size check cannot cause an unbounded
/// allocation. Special files are refused; on Unix nonblocking open also prevents a FIFO
/// replacement from waiting for a writer before its type can be checked.
pub(crate) fn require_bounded_content_input(
    policy: &PathPolicy,
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
            let resolved = policy.check_read(path)?;
            let (file, metadata) = open_regular_file(&resolved, field, path)?;
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

    /// Runs the helper against the file path `path` under the test policy.
    fn from_path(path: &std::path::Path, cap: u64) -> Result<String> {
        require_bounded_content_input(
            &PathPolicy::for_tests(),
            None,
            Some(path.to_str().unwrap()),
            "text",
            cap,
        )
    }

    fn inline(text: &str, path: Option<&str>, cap: u64) -> Result<String> {
        require_bounded_content_input(&PathPolicy::for_tests(), Some(text), path, "text", cap)
    }

    #[test]
    fn bounded_input_accepts_the_cap_and_rejects_larger_or_invalid_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text.txt");
        std::fs::write(&path, "abcd").unwrap();
        assert_eq!(from_path(&path, 4).unwrap(), "abcd");
        assert_eq!(inline("abcd", None, 4).unwrap(), "abcd");
        assert!(inline("abcde", None, 4)
            .unwrap_err()
            .to_string()
            .contains("cap"));
        assert!(from_path(&path, 3).unwrap_err().to_string().contains("cap"));
        std::fs::write(&path, [0xff]).unwrap();
        assert!(from_path(&path, 4)
            .unwrap_err()
            .to_string()
            .contains("UTF-8"));
        assert!(inline("x", Some("missing"), 4)
            .unwrap_err()
            .to_string()
            .contains("not both"));
    }

    #[test]
    fn bounded_input_needs_one_source_and_rejects_stdin() {
        let policy = PathPolicy::for_tests();
        let err = require_bounded_content_input(&policy, None, None, "text", 4).unwrap_err();
        assert!(err.to_string().contains("Provide either"), "{err}");
        let err = require_bounded_content_input(&policy, None, Some("-"), "text", 4).unwrap_err();
        assert!(err.to_string().contains("stdin"), "{err}");
    }

    #[test]
    fn bounded_input_refuses_a_path_outside_the_policy_before_reading_it() {
        let allowed = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "do not upload").unwrap();
        let policy = PathPolicy::allowing_only(allowed.path());

        let err = require_bounded_content_input(
            &policy,
            None,
            Some(secret.to_str().unwrap()),
            "text",
            1024,
        )
        .unwrap_err();

        assert!(err.to_string().contains("outside the allowed"), "{err}");
        // The refusal names the policy, not a read failure, so the file was never opened.
        assert!(!format!("{err:#}").contains("Failed to open"), "{err:#}");
    }

    #[test]
    fn bounded_input_refuses_a_credential_file_even_when_its_directory_is_allowed() {
        // `~/.ssh` is not built from the real home here: a fake home with an allowed
        // root above it is enough to prove the deny list outranks the allowed set.
        let home = tempfile::tempdir().unwrap();
        let key = home.path().join(".ssh/id_rsa");
        std::fs::create_dir_all(key.parent().unwrap()).unwrap();
        std::fs::write(&key, "private").unwrap();
        let settings = crate::utils::settings::McpSettings {
            allowed_paths: Some(vec![home.path().to_str().unwrap().to_string()]),
            ..Default::default()
        };
        let dirs = super::super::path_policy::PolicyDirs {
            cwd: None,
            temp: home.path().join("tmp"),
            home: Some(home.path().canonicalize().unwrap()),
            state: None,
        };
        let policy = PathPolicy::from_settings(&settings, &dirs).unwrap();

        let err =
            require_bounded_content_input(&policy, None, Some(key.to_str().unwrap()), "text", 1024)
                .unwrap_err();

        assert!(err.to_string().contains("protected credential"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn bounded_input_refuses_a_symlink_that_leaves_the_allowed_directory() {
        let allowed = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "do not upload").unwrap();
        let link = allowed.path().join("innocent.txt");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        let policy = PathPolicy::allowing_only(allowed.path());

        let err =
            require_bounded_content_input(&policy, None, Some(link.to_str().unwrap()), "text", 64)
                .unwrap_err();

        assert!(err.to_string().contains("outside the allowed"), "{err}");
    }

    #[test]
    fn bounded_input_refuses_dot_dot_traversal_out_of_the_allowed_directory() {
        let allowed = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "do not upload").unwrap();
        let policy = PathPolicy::allowing_only(allowed.path());
        // From the allowed directory up to the filesystem root and down to the secret.
        let depth = allowed.path().components().count();
        let traversal = format!(
            "{}{}{}",
            allowed.path().display(),
            "/..".repeat(depth),
            secret.display()
        );

        let err =
            require_bounded_content_input(&policy, None, Some(&traversal), "text", 64).unwrap_err();

        assert!(err.to_string().contains("outside the allowed"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn bounded_input_refuses_fifos_without_reading_them() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe");
        nix::unistd::mkfifo(
            &fifo,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .unwrap();
        assert!(from_path(&fifo, 4)
            .unwrap_err()
            .to_string()
            .contains("regular"));
    }

    #[cfg(unix)]
    #[test]
    fn open_regular_file_refuses_devices() {
        let err =
            open_regular_file(std::path::Path::new("/dev/zero"), "text", "/dev/zero").unwrap_err();
        assert!(err.to_string().contains("regular"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn open_regular_file_clears_nonblocking_once_the_file_is_known_regular() {
        use nix::fcntl::{fcntl, FcntlArg, OFlag};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text.txt");
        std::fs::write(&path, "abcd").unwrap();

        let (file, metadata) = open_regular_file(&path, "text", "text.txt").unwrap();

        assert_eq!(metadata.len(), 4);
        let flags = OFlag::from_bits_retain(fcntl(&file, FcntlArg::F_GETFL).unwrap());
        assert!(!flags.contains(OFlag::O_NONBLOCK), "{flags:?}");
    }
}
