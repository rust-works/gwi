//! Operator policy for the local paths MCP tool callers choose (#45).
//!
//! Two parameters name a caller-chosen local path: the `*_path` sources of the Drive
//! write tools (read, then written into a Google Doc or Sheet) and `output_file` on the
//! read tools (written to disk). Both are driven by an assistant that can be
//! prompt-injected, so left unchecked they are a read-a-file-and-upload channel
//! (`text_path: "~/.ssh/id_rsa"`) and an arbitrary-file write.
//!
//! `PathPolicy` closes both. A path is resolved first (symlinks followed, `..`
//! collapsed) and the *resolved* path is tested, so a link or a traversal cannot leave
//! the allowed set. The test is component-wise [`Path::starts_with`], never a string
//! prefix. Two lists are consulted, in this order:
//!
//! 1. A fixed deny list of credential locations (`~/.ssh`, `~/.gwi`, gwi's own state
//!    directory with the lease ledger and `audit.jsonl`, ...). It always wins, even over
//!    an explicit `mcp.allowed_paths` entry.
//! 2. The allowed directories: `mcp.allowed_paths` from `settings.json` when set,
//!    otherwise the server's working directory and the system temp directory.
//!
//! The policy is operator-only; no tool parameter can change it. The CLI is not
//! restricted: its paths are chosen by the person at the terminal.
//!
//! Callers use the *resolved* path for the I/O that follows, which keeps the window
//! between check and use small. It is not closed: a local attacker who can swap a
//! directory component inside an allowed directory between the two calls is out of scope
//! (it needs write access to that directory already). Writes additionally refuse a final
//! symlink, a FIFO or a device found at open time.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::utils::settings::{McpSettings, Settings};

/// Credential locations under the home directory that are refused whatever the allowed
/// set says.
const PROTECTED_UNDER_HOME: &[&str] = &[
    // gwi's and omni-dev's own settings, tokens and ledgers.
    ".gwi",
    ".omni-dev",
    ".config/gwi",
    // Credential stores.
    ".ssh",
    ".gnupg",
    ".aws",
    ".kube",
    ".docker",
    ".netrc",
    ".git-credentials",
    ".npmrc",
    ".pypirc",
    ".config/gcloud",
    ".config/gh",
    ".local/share/keyrings",
    "Library/Keychains",
    // Files a shell or git runs on its own, so a write is persistent code execution.
    ".zshenv",
    ".zprofile",
    ".zshrc",
    ".zlogin",
    ".bash_profile",
    ".bash_login",
    ".bashrc",
    ".profile",
    ".gitconfig",
    // Shell history regularly holds pasted secrets.
    ".bash_history",
    ".zsh_history",
];

/// The directories a policy is built from, passed in rather than read from the
/// environment so tests do not touch `HOME` (STYLE-0028).
#[derive(Debug, Clone)]
pub(crate) struct PolicyDirs {
    /// The server's working directory.
    pub cwd: Option<PathBuf>,
    /// The system temp directory.
    pub temp: PathBuf,
    /// The user's home directory.
    pub home: Option<PathBuf>,
    /// The base under which gwi keeps runtime state (`state_dir`, else `data_dir`).
    pub state: Option<PathBuf>,
}

impl PolicyDirs {
    /// The directories of the running process.
    fn current() -> Self {
        Self {
            cwd: std::env::current_dir().ok(),
            temp: std::env::temp_dir(),
            home: dirs::home_dir(),
            state: dirs::state_dir().or_else(dirs::data_dir),
        }
    }
}

/// Resolved allowed and protected locations for caller-chosen local paths.
#[derive(Debug, Clone)]
pub(crate) struct PathPolicy {
    allowed: Vec<PathBuf>,
    protected: Vec<PathBuf>,
    /// Why the settings could not be turned into a policy; every path is refused with it.
    unusable: Option<String>,
}

impl PathPolicy {
    /// Builds the policy of the running process from the `mcp` block of `settings.json`.
    ///
    /// Never fails: settings that cannot form a policy (a `settings.json` that does not
    /// parse, an `allowed_paths` entry that is not absolute) give a policy that refuses
    /// every path and says why, so the tools that take no path keep working and the ones
    /// that do fail closed.
    pub(crate) fn load() -> Self {
        // Not `load_mcp`: that falls back to defaults on a settings file it cannot
        // parse, which would widen a configured `allowed_paths` to the default roots.
        Self::from_load_result(Settings::load(), &PolicyDirs::current())
    }

    /// [`Self::load`] with the settings read and the directories injected.
    fn from_load_result(settings: Result<Settings>, dirs: &PolicyDirs) -> Self {
        match settings {
            Ok(settings) => Self::from_settings(&settings.mcp, dirs)
                .unwrap_or_else(|err| Self::unusable(format!("{err:#}"))),
            Err(err) => Self::unusable(format!("settings.json could not be read: {err:#}")),
        }
    }

    /// A policy that refuses every path, giving `reason`.
    fn unusable(reason: String) -> Self {
        Self {
            allowed: Vec::new(),
            protected: Vec::new(),
            unusable: Some(reason),
        }
    }

    /// Builds a policy from explicit settings and directories.
    ///
    /// Fails when an `allowed_paths` entry is neither absolute nor `~/`-relative, so a
    /// typo cannot silently widen or drop the allowed set.
    pub(crate) fn from_settings(settings: &McpSettings, dirs: &PolicyDirs) -> Result<Self> {
        let mut allowed = Vec::new();
        if let Some(entries) = &settings.allowed_paths {
            for entry in entries {
                let expanded = expand_entry(entry, dirs.home.as_deref())?;
                match expanded.canonicalize() {
                    Ok(resolved) => allowed.push(resolved),
                    // A directory that does not exist allows nothing; say so, because an
                    // operator who sees only "outside the allowed directories" would not
                    // know the entry was ignored.
                    Err(err) => tracing::warn!(
                        "mcp.allowed_paths entry {entry:?} was ignored: cannot resolve {}: {err}",
                        expanded.display()
                    ),
                }
            }
        } else {
            let home = dirs.home.as_deref().and_then(|dir| dir.canonicalize().ok());
            if let Some(cwd) = dirs.cwd.as_deref().and_then(|dir| dir.canonicalize().ok()) {
                // A client that launches the server in `/` or in the home directory
                // would otherwise hand the model that whole tree.
                let too_broad = cwd.parent().is_none()
                    || home.as_deref().is_some_and(|home| home.starts_with(&cwd));
                if too_broad {
                    tracing::warn!(
                        "the working directory {} is too broad to allow by default; set \
                         mcp.allowed_paths to use paths outside the temp directory",
                        cwd.display()
                    );
                } else {
                    allowed.push(cwd);
                }
            }
            allowed.extend(dirs.temp.canonicalize().ok());
        }

        let mut protected = Vec::new();
        if let Some(home) = &dirs.home {
            for name in PROTECTED_UNDER_HOME {
                push_protected(&mut protected, &home.join(name));
            }
        }
        if let Some(state) = &dirs.state {
            push_protected(&mut protected, &state.join("gwi"));
        }
        // A project-local `.gwi/` configuration directory under the working directory.
        if let Some(cwd) = &dirs.cwd {
            push_protected(&mut protected, &cwd.join(".gwi"));
        }
        Ok(Self {
            allowed,
            protected,
            unusable: None,
        })
    }

    /// A policy for tests of the tools that use one: the system temp directory is
    /// allowed (where `tempfile` creates its directories) and every protected location
    /// under the real home is refused.
    #[cfg(test)]
    #[allow(clippy::expect_used)]
    pub(crate) fn for_tests() -> Self {
        Self::from_settings(&McpSettings::default(), &PolicyDirs::current())
            .expect("the default policy always builds")
    }

    /// A test policy that allows exactly one directory.
    #[cfg(test)]
    #[allow(clippy::expect_used)]
    pub(crate) fn allowing_only(dir: &Path) -> Self {
        let settings = McpSettings {
            allowed_paths: Some(vec![dir.to_str().expect("UTF-8 test path").to_string()]),
            ..McpSettings::default()
        };
        Self::from_settings(&settings, &PolicyDirs::current()).expect("an absolute entry")
    }

    /// Resolves a path to read and checks it against the policy.
    ///
    /// The file must exist. Returns the resolved path, which the caller must open in
    /// place of the one it was given.
    pub(crate) fn check_read(&self, path: &str) -> Result<PathBuf> {
        let resolved = Path::new(path)
            .canonicalize()
            .with_context(|| format!("Failed to resolve {path}"))?;
        self.authorize(path, &resolved, "read")?;
        Ok(resolved)
    }

    /// Resolves a path to write and checks it against the policy.
    ///
    /// The file may not exist yet, but its directory must. An existing target is
    /// resolved through any symlink, so a link pointing out of the allowed set is
    /// refused. Returns the resolved path, which the caller must open in place of the
    /// one it was given.
    pub(crate) fn check_write(&self, path: &str) -> Result<PathBuf> {
        let resolved = resolve_for_write(Path::new(path))
            .with_context(|| format!("Failed to resolve {path}"))?;
        self.authorize(path, &resolved, "write")?;
        Ok(resolved)
    }

    /// Writes `bytes` to the resolved, authorised form of `path`.
    ///
    /// On Unix the file is opened without following a final symlink and without blocking,
    /// and only truncated once the open handle is known to be a regular file, so a FIFO or
    /// device swapped in after the check is refused rather than opened or waited on.
    pub(crate) fn write(&self, path: &str, bytes: &[u8]) -> Result<()> {
        use std::io::Write as _;

        let resolved = self.check_write(path)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.custom_flags(nix::libc::O_NONBLOCK | nix::libc::O_NOFOLLOW);
        }
        #[cfg(not(unix))]
        options.truncate(true);
        let mut file = options
            .open(&resolved)
            .with_context(|| format!("Failed to write to {path}"))?;
        anyhow::ensure!(
            file.metadata()
                .with_context(|| format!("Failed to stat {path}"))?
                .is_file(),
            "Refusing to write to {path}: it is not a regular file"
        );
        #[cfg(unix)]
        {
            file.set_len(0)
                .with_context(|| format!("Failed to truncate {path}"))?;
            clear_nonblocking(&file)
                .with_context(|| format!("Failed to clear O_NONBLOCK on {path}"))?;
        }
        file.write_all(bytes)
            .with_context(|| format!("Failed to write to {path}"))
    }

    fn authorize(&self, given: &str, resolved: &Path, verb: &str) -> Result<()> {
        if let Some(reason) = &self.unusable {
            anyhow::bail!("Refusing to {verb} {given}: the path policy is unusable: {reason}");
        }
        anyhow::ensure!(
            !self.protected.iter().any(|dir| resolved.starts_with(dir)),
            "Refusing to {verb} {given}: it resolves to {}, a protected credential location",
            resolved.display()
        );
        if self.allowed.iter().any(|dir| resolved.starts_with(dir)) {
            return Ok(());
        }
        let allowed = if self.allowed.is_empty() {
            "none".to_string()
        } else {
            self.allowed
                .iter()
                .map(|dir| dir.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        anyhow::bail!(
            "Refusing to {verb} {given}: it resolves to {}, outside the allowed directories \
             ({allowed}). An operator can change them with `mcp.allowed_paths` in settings.json.",
            resolved.display()
        )
    }
}

/// Clears `O_NONBLOCK` on an open file description.
///
/// The flag is set only so that opening a FIFO cannot block; once the handle is known to
/// be a regular file, a leftover flag can turn a read or write on some FUSE and network
/// mounts into a confusing `EAGAIN` error.
#[cfg(unix)]
pub(crate) fn clear_nonblocking(file: &std::fs::File) -> nix::Result<()> {
    use nix::fcntl::{fcntl, FcntlArg, OFlag};

    let flags = OFlag::from_bits_retain(fcntl(file, FcntlArg::F_GETFL)?);
    fcntl(file, FcntlArg::F_SETFL(flags & !OFlag::O_NONBLOCK)).map(|_| ())
}

/// Expands one `allowed_paths` entry: absolute, or `~/…` against the home directory.
fn expand_entry(entry: &str, home: Option<&Path>) -> Result<PathBuf> {
    let expanded = match entry.strip_prefix("~/") {
        Some(rest) => home
            .context("Cannot expand `~/` in `mcp.allowed_paths`: no home directory")?
            .join(rest),
        None => PathBuf::from(entry),
    };
    anyhow::ensure!(
        expanded.is_absolute(),
        "`mcp.allowed_paths` entry {entry:?} must be an absolute path or start with `~/`"
    );
    Ok(expanded)
}

/// Records a protected location in both its lexical and its resolved form, so a symlink
/// standing in for it (`~/.ssh -> /secrets/ssh`) is protected under either name.
fn push_protected(protected: &mut Vec<PathBuf>, path: &Path) {
    protected.push(path.to_path_buf());
    if let Ok(resolved) = path.canonicalize() {
        if resolved != path {
            protected.push(resolved);
        }
    }
    // The home directory itself may sit behind a link (`/home -> /var/home`).
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        if let Ok(parent) = parent.canonicalize() {
            let joined = parent.join(name);
            if !protected.contains(&joined) {
                protected.push(joined);
            }
        }
    }
}

/// Resolves a path that may not exist yet: an existing entry is canonicalised (following
/// a symlink, and failing for a dangling one); a new file is its canonical parent plus
/// its final component.
fn resolve_for_write(path: &Path) -> std::io::Result<PathBuf> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => path.canonicalize(),
        Err(err) if err.kind() == ErrorKind::NotFound => {
            let name = path.file_name().ok_or_else(|| {
                std::io::Error::new(ErrorKind::InvalidInput, "no file name in the path")
            })?;
            let parent = match path.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => parent,
                _ => Path::new("."),
            };
            Ok(parent.canonicalize()?.join(name))
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A sandbox with a home, an allowed directory and an unrelated directory, so no
    /// test reads the process environment.
    struct Sandbox {
        root: tempfile::TempDir,
        home: PathBuf,
        work: PathBuf,
        elsewhere: PathBuf,
        state: PathBuf,
    }

    impl Sandbox {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let base = root.path().canonicalize().unwrap();
            let sandbox = Self {
                home: base.join("home"),
                work: base.join("work"),
                elsewhere: base.join("elsewhere"),
                state: base.join("state"),
                root,
            };
            for dir in [
                &sandbox.home,
                &sandbox.work,
                &sandbox.elsewhere,
                &sandbox.state,
            ] {
                std::fs::create_dir_all(dir).unwrap();
            }
            sandbox
        }

        fn dirs(&self) -> PolicyDirs {
            PolicyDirs {
                cwd: Some(self.work.clone()),
                // Not an ancestor of the sandbox, so the temp default never covers it.
                temp: self.work.join("tmp"),
                home: Some(self.home.clone()),
                state: Some(self.state.clone()),
            }
        }

        fn policy(&self) -> PathPolicy {
            PathPolicy::from_settings(&McpSettings::default(), &self.dirs()).unwrap()
        }

        fn policy_allowing(&self, entries: &[&str]) -> PathPolicy {
            let settings = McpSettings {
                allowed_paths: Some(entries.iter().map(|e| (*e).to_string()).collect()),
                ..McpSettings::default()
            };
            PathPolicy::from_settings(&settings, &self.dirs()).unwrap()
        }

        fn file_in(&self, dir: &Path, name: &str) -> String {
            let path = dir.join(name);
            std::fs::write(&path, "content").unwrap();
            path.to_str().unwrap().to_string()
        }
    }

    fn refusal(result: Result<PathBuf>) -> String {
        format!("{:#}", result.unwrap_err())
    }

    #[test]
    fn a_file_in_the_default_roots_is_allowed() {
        let sandbox = Sandbox::new();
        std::fs::create_dir(sandbox.work.join("tmp")).unwrap();
        let policy = sandbox.policy();

        let in_cwd = sandbox.file_in(&sandbox.work, "a.txt");
        let in_temp = sandbox.file_in(&sandbox.work.join("tmp"), "b.txt");
        assert_eq!(
            policy.check_read(&in_cwd).unwrap(),
            sandbox.work.join("a.txt")
        );
        policy.check_read(&in_temp).unwrap();
        policy.check_write(&in_cwd).unwrap();
    }

    #[test]
    fn a_path_outside_the_allowed_set_is_refused_with_the_setting_named() {
        let sandbox = Sandbox::new();
        let outside = sandbox.file_in(&sandbox.elsewhere, "a.txt");
        let policy = sandbox.policy();

        for result in [policy.check_read(&outside), policy.check_write(&outside)] {
            let message = refusal(result);
            assert!(
                message.contains("outside the allowed directories"),
                "{message}"
            );
            assert!(message.contains("mcp.allowed_paths"), "{message}");
        }
    }

    #[test]
    fn dot_dot_traversal_out_of_an_allowed_directory_is_refused() {
        let sandbox = Sandbox::new();
        sandbox.file_in(&sandbox.elsewhere, "a.txt");
        let policy = sandbox.policy();
        // Use the original temp path: canonical Windows paths carry a verbatim
        // prefix, which deliberately disables normalisation of `..`.
        let traversal = sandbox.root.path().join("work/../elsewhere/a.txt");
        let traversal = traversal.to_str().unwrap();

        assert!(refusal(policy.check_read(traversal)).contains("outside"));
        assert!(refusal(policy.check_write(traversal)).contains("outside"));
        // A new file through a traversal is resolved through its parent.
        let new_file = sandbox.root.path().join("work/../elsewhere/new.txt");
        assert!(refusal(policy.check_write(new_file.to_str().unwrap())).contains("outside"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_an_allowed_directory_is_refused() {
        let sandbox = Sandbox::new();
        let target = sandbox.file_in(&sandbox.elsewhere, "secret.txt");
        let link = sandbox.work.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let link_dir = sandbox.work.join("linkdir");
        std::os::unix::fs::symlink(&sandbox.elsewhere, &link_dir).unwrap();
        let policy = sandbox.policy();

        // The link is inside the allowed directory; what it points to is not.
        assert!(refusal(policy.check_read(link.to_str().unwrap())).contains("outside"));
        assert!(refusal(policy.check_write(link.to_str().unwrap())).contains("outside"));
        // A symlinked directory component, for an existing and for a new file.
        let through_dir = link_dir.join("secret.txt");
        assert!(refusal(policy.check_read(through_dir.to_str().unwrap())).contains("outside"));
        let new_through_dir = link_dir.join("new.txt");
        assert!(refusal(policy.check_write(new_through_dir.to_str().unwrap())).contains("outside"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_that_stays_inside_the_allowed_set_is_followed_and_allowed() {
        let sandbox = Sandbox::new();
        let target = sandbox.file_in(&sandbox.work, "real.txt");
        let link = sandbox.work.join("alias.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let resolved = sandbox.policy().check_read(link.to_str().unwrap()).unwrap();
        assert_eq!(resolved, sandbox.work.join("real.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_symlink_is_refused_for_writing() {
        let sandbox = Sandbox::new();
        let link = sandbox.work.join("dangling.txt");
        std::os::unix::fs::symlink(sandbox.elsewhere.join("absent.txt"), &link).unwrap();

        // Writing through it would create the file outside the allowed set.
        assert!(sandbox
            .policy()
            .check_write(link.to_str().unwrap())
            .is_err());
    }

    #[test]
    fn credential_locations_are_refused_even_when_explicitly_allowed() {
        let sandbox = Sandbox::new();
        let home = sandbox.home.to_str().unwrap();
        // The operator allows the whole home directory; the deny list still wins.
        let policy = sandbox.policy_allowing(&[home]);

        for location in [
            ".config/gwi/settings.json",
            ".zshrc",
            ".bashrc",
            ".profile",
            ".gitconfig",
            ".git-credentials",
            ".npmrc",
            ".config/gh/hosts.yml",
            ".zsh_history",
            ".gwi/settings.json",
            ".omni-dev/settings.json",
            ".ssh/id_rsa",
            ".gnupg/private-keys",
            ".aws/credentials",
            ".kube/config",
            ".docker/config.json",
            ".netrc",
            ".config/gcloud/credentials.db",
            "Library/Keychains/login.keychain-db",
        ] {
            let path = sandbox.home.join(location);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "secret").unwrap();
            let path = path.to_str().unwrap();
            for result in [policy.check_read(path), policy.check_write(path)] {
                let message = refusal(result);
                assert!(
                    message.contains("protected credential location"),
                    "{location}: {message}"
                );
            }
        }
        // Outside the protected names the same directory is usable.
        let fine = sandbox.file_in(&sandbox.home, "notes.txt");
        policy.check_read(&fine).unwrap();
    }

    #[test]
    fn gwi_state_directory_is_protected() {
        let sandbox = Sandbox::new();
        let audit = sandbox.state.join("gwi/audit.jsonl");
        std::fs::create_dir_all(audit.parent().unwrap()).unwrap();
        std::fs::write(&audit, "{}").unwrap();
        let policy = sandbox.policy_allowing(&[sandbox.state.to_str().unwrap()]);

        let message = refusal(policy.check_write(audit.to_str().unwrap()));
        assert!(
            message.contains("protected credential location"),
            "{message}"
        );
        // A missing file under it is protected too.
        let ledger = sandbox.state.join("gwi/lease-ledger.jsonl");
        assert!(policy.check_write(ledger.to_str().unwrap()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_into_a_credential_location_is_refused() {
        let sandbox = Sandbox::new();
        let key = sandbox.home.join(".ssh/id_rsa");
        std::fs::create_dir_all(key.parent().unwrap()).unwrap();
        std::fs::write(&key, "private").unwrap();
        let link = sandbox.work.join("innocent.txt");
        std::os::unix::fs::symlink(&key, &link).unwrap();
        // Allow both, so only the deny list can refuse.
        let policy = sandbox.policy_allowing(&[
            sandbox.work.to_str().unwrap(),
            sandbox.home.to_str().unwrap(),
        ]);

        let message = refusal(policy.check_read(link.to_str().unwrap()));
        assert!(
            message.contains("protected credential location"),
            "{message}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_protected_directory_behind_a_symlink_is_protected_under_its_target() {
        let sandbox = Sandbox::new();
        let real = sandbox.elsewhere.join("real-ssh");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("id_rsa"), "private").unwrap();
        std::os::unix::fs::symlink(&real, sandbox.home.join(".ssh")).unwrap();
        // The allowed set covers the link's target directly.
        let policy = sandbox.policy_allowing(&[sandbox.elsewhere.to_str().unwrap()]);

        let message = refusal(policy.check_read(real.join("id_rsa").to_str().unwrap()));
        assert!(
            message.contains("protected credential location"),
            "{message}"
        );
    }

    #[test]
    fn allowed_paths_replace_the_defaults_and_empty_allows_nothing() {
        let sandbox = Sandbox::new();
        let in_cwd = sandbox.file_in(&sandbox.work, "a.txt");
        let in_other = sandbox.file_in(&sandbox.elsewhere, "b.txt");

        let only_other = sandbox.policy_allowing(&[sandbox.elsewhere.to_str().unwrap()]);
        only_other.check_read(&in_other).unwrap();
        assert!(
            only_other.check_read(&in_cwd).is_err(),
            "cwd is no longer allowed"
        );

        let nothing = sandbox.policy_allowing(&[]);
        assert!(nothing.check_read(&in_cwd).is_err());
        assert!(refusal(nothing.check_read(&in_cwd)).contains("(none)"));
    }

    #[test]
    fn a_sibling_that_shares_a_string_prefix_is_not_inside_the_root() {
        let sandbox = Sandbox::new();
        let sibling = sandbox.work.with_file_name("work-other");
        std::fs::create_dir_all(&sibling).unwrap();
        let outside = sandbox.file_in(&sibling, "a.txt");

        assert!(sandbox.policy().check_read(&outside).is_err());
    }

    #[test]
    fn allowed_entries_expand_a_tilde_and_must_be_absolute() {
        let sandbox = Sandbox::new();
        std::fs::create_dir_all(sandbox.home.join("out")).unwrap();
        let file = sandbox.file_in(&sandbox.home.join("out"), "a.txt");
        sandbox
            .policy_allowing(&["~/out"])
            .check_read(&file)
            .unwrap();

        for bad in ["relative/dir", "./here", ""] {
            let settings = McpSettings {
                allowed_paths: Some(vec![bad.to_string()]),
                ..McpSettings::default()
            };
            let err = PathPolicy::from_settings(&settings, &sandbox.dirs()).unwrap_err();
            assert!(err.to_string().contains("absolute"), "{bad:?}: {err}");
        }

        let mut no_home = sandbox.dirs();
        no_home.home = None;
        let settings = McpSettings {
            allowed_paths: Some(vec!["~/out".to_string()]),
            ..McpSettings::default()
        };
        assert!(PathPolicy::from_settings(&settings, &no_home).is_err());
    }

    #[test]
    fn a_missing_allowed_directory_allows_nothing() {
        let sandbox = Sandbox::new();
        let missing = sandbox.elsewhere.join("absent");
        let file = sandbox.file_in(&sandbox.elsewhere, "a.txt");

        assert!(sandbox
            .policy_allowing(&[missing.to_str().unwrap()])
            .check_read(&file)
            .is_err());
    }

    #[test]
    fn a_new_file_needs_an_existing_allowed_directory() {
        let sandbox = Sandbox::new();
        let policy = sandbox.policy();

        let new_file = sandbox.work.join("fresh.txt");
        assert_eq!(
            policy.check_write(new_file.to_str().unwrap()).unwrap(),
            new_file
        );
        let in_missing_dir = sandbox.work.join("missing/fresh.txt");
        assert!(policy
            .check_write(in_missing_dir.to_str().unwrap())
            .is_err());
        // No final component to create.
        assert!(policy.check_write("").is_err());
        assert!(policy
            .check_write(&format!("{}/..", sandbox.work.display()))
            .is_err());
    }

    #[test]
    fn reading_a_missing_file_reports_the_path() {
        let sandbox = Sandbox::new();
        let missing = sandbox.work.join("absent.txt");
        let message = refusal(sandbox.policy().check_read(missing.to_str().unwrap()));
        assert!(message.contains("absent.txt"), "{message}");
    }

    #[test]
    fn write_creates_and_overwrites_inside_the_allowed_set_only() {
        let sandbox = Sandbox::new();
        let policy = sandbox.policy();
        let target = sandbox.work.join("out.txt");
        let target_str = target.to_str().unwrap();

        policy.write(target_str, b"one").unwrap();
        policy.write(target_str, b"two").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "two");

        let outside = sandbox.elsewhere.join("out.txt");
        assert!(policy.write(outside.to_str().unwrap(), b"x").is_err());
        assert!(
            !outside.exists(),
            "a refused write must not create the file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_leaves_the_target_of_a_symlink_out_of_the_set_untouched() {
        let sandbox = Sandbox::new();
        let target = sandbox.elsewhere.join("victim.txt");
        std::fs::write(&target, "original").unwrap();
        let link = sandbox.work.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(sandbox
            .policy()
            .write(link.to_str().unwrap(), b"overwritten")
            .is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
    }

    #[test]
    fn a_project_local_gwi_directory_under_the_working_directory_is_protected() {
        let sandbox = Sandbox::new();
        let local = sandbox.work.join(".gwi/settings.json");
        std::fs::create_dir_all(local.parent().unwrap()).unwrap();
        std::fs::write(&local, "{}").unwrap();

        let message = refusal(sandbox.policy().check_read(local.to_str().unwrap()));
        assert!(
            message.contains("protected credential location"),
            "{message}"
        );
    }

    #[test]
    fn a_working_directory_that_is_the_root_the_home_or_above_it_is_not_allowed() {
        let sandbox = Sandbox::new();
        let file = sandbox.file_in(&sandbox.home, "notes.txt");
        let elsewhere = sandbox.file_in(&sandbox.elsewhere, "a.txt");
        let above_home = sandbox.home.parent().unwrap().to_path_buf();

        for cwd in [PathBuf::from("/"), sandbox.home.clone(), above_home] {
            let mut dirs = sandbox.dirs();
            dirs.cwd = Some(cwd.clone());
            let policy = PathPolicy::from_settings(&McpSettings::default(), &dirs).unwrap();
            for path in [&file, &elsewhere] {
                let message = refusal(policy.check_read(path));
                assert!(
                    message.contains("outside the allowed"),
                    "{cwd:?}: {message}"
                );
            }
        }
    }

    #[test]
    fn an_explicit_allowed_path_may_still_name_the_home_directory() {
        let sandbox = Sandbox::new();
        let file = sandbox.file_in(&sandbox.home, "notes.txt");

        sandbox
            .policy_allowing(&[sandbox.home.to_str().unwrap()])
            .check_read(&file)
            .unwrap();
    }

    #[test]
    fn settings_that_cannot_be_read_give_a_policy_that_refuses_everything() {
        let sandbox = Sandbox::new();
        let file = sandbox.file_in(&sandbox.work, "a.txt");

        let policy =
            PathPolicy::from_load_result(Err(anyhow::anyhow!("trailing comma")), &sandbox.dirs());

        let message = refusal(policy.check_read(&file));
        assert!(
            message.contains("could not be read: trailing comma"),
            "{message}"
        );
        let ok = PathPolicy::from_load_result(Ok(Settings::default()), &sandbox.dirs());
        ok.check_read(&file).unwrap();
        let bad: Settings =
            serde_json::from_str(r#"{"mcp": {"allowed_paths": ["relative"]}}"#).unwrap();
        let policy = PathPolicy::from_load_result(Ok(bad), &sandbox.dirs());
        assert!(refusal(policy.check_read(&file)).contains("unusable"));
    }

    /// A `Write` that appends to a shared buffer, so a test can read what was logged.
    #[derive(Clone, Default)]
    struct LogBuffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for LogBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Runs `action` with a `warn`-level subscriber installed and returns what it logged.
    /// The arguments of a disabled `tracing` event are never evaluated, so the lines that
    /// format them only run under a subscriber.
    fn logged_during(action: impl FnOnce()) -> String {
        let buffer = LogBuffer::default();
        let writer = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, action);
        let bytes = buffer.0.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn an_ignored_allowed_directory_is_logged_with_its_path() {
        let sandbox = Sandbox::new();
        let missing = sandbox.elsewhere.join("absent");

        let log = logged_during(|| {
            sandbox.policy_allowing(&[missing.to_str().unwrap()]);
        });

        assert!(log.contains("was ignored: cannot resolve"), "{log}");
        assert!(log.contains(missing.to_str().unwrap()), "{log}");
    }

    #[test]
    fn a_working_directory_that_is_too_broad_is_logged() {
        let sandbox = Sandbox::new();
        let mut dirs = sandbox.dirs();
        dirs.cwd = Some(sandbox.home.clone());

        let log = logged_during(|| {
            PathPolicy::from_settings(&McpSettings::default(), &dirs).unwrap();
        });

        assert!(log.contains("is too broad to allow by default"), "{log}");
        assert!(log.contains(sandbox.home.to_str().unwrap()), "{log}");
    }

    #[test]
    fn a_policy_without_a_working_directory_or_a_home_still_allows_the_temp_directory() {
        let sandbox = Sandbox::new();
        let temp = sandbox.elsewhere.clone();
        let file = sandbox.file_in(&temp, "a.txt");
        let dirs = PolicyDirs {
            cwd: None,
            temp,
            home: None,
            state: None,
        };

        let policy = PathPolicy::from_settings(&McpSettings::default(), &dirs).unwrap();

        policy.check_read(&file).unwrap();
        assert!(policy
            .check_read(&sandbox.file_in(&sandbox.work, "b.txt"))
            .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_protected_location_under_a_linked_parent_is_protected_under_its_target() {
        let sandbox = Sandbox::new();
        // `link -> home`, so a home of `<link>` resolves to `<home>`.
        let link = sandbox.elsewhere.join("link");
        std::os::unix::fs::symlink(&sandbox.home, &link).unwrap();
        // Not created: only the link's target names it, since there is nothing to resolve.
        let ssh = sandbox.home.join(".ssh");
        let mut dirs = sandbox.dirs();
        dirs.home = Some(link);
        let policy = PathPolicy::from_settings(
            &McpSettings {
                allowed_paths: Some(vec![sandbox.home.to_str().unwrap().to_string()]),
                ..McpSettings::default()
            },
            &dirs,
        )
        .unwrap();

        let message = refusal(policy.check_write(ssh.to_str().unwrap()));

        assert!(
            message.contains("protected credential location"),
            "{message}"
        );
    }

    #[test]
    fn resolve_for_write_puts_a_bare_file_name_in_the_current_directory() {
        let resolved = resolve_for_write(Path::new("gwi-no-such-file.tmp")).unwrap();

        assert_eq!(
            resolved,
            std::env::current_dir()
                .unwrap()
                .canonicalize()
                .unwrap()
                .join("gwi-no-such-file.tmp")
        );
    }

    // Pins Unix's NotADirectory error; Windows reports a child of a file as NotFound.
    #[cfg(unix)]
    #[test]
    fn resolve_for_write_passes_on_an_error_that_is_not_a_missing_file() {
        let sandbox = Sandbox::new();
        let file = sandbox.file_in(&sandbox.work, "plain.txt");

        // A file used as a directory is `NotADirectory`, not `NotFound`.
        let err = resolve_for_write(&Path::new(&file).join("child")).unwrap_err();

        assert_ne!(err.kind(), ErrorKind::NotFound);
    }

    #[test]
    fn write_truncates_a_longer_existing_file() {
        let sandbox = Sandbox::new();
        let target = sandbox.work.join("out.txt");
        std::fs::write(&target, "a much longer previous body").unwrap();

        sandbox
            .policy()
            .write(target.to_str().unwrap(), b"short")
            .unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "short");
    }

    #[cfg(unix)]
    #[test]
    fn write_refuses_an_existing_fifo_without_opening_it() {
        let sandbox = Sandbox::new();
        let fifo = sandbox.work.join("pipe");
        nix::unistd::mkfifo(
            &fifo,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .unwrap();

        // Opening a FIFO for writing without a reader fails at once rather than blocking.
        assert!(sandbox
            .policy()
            .write(fifo.to_str().unwrap(), b"x")
            .is_err());
    }

    #[test]
    fn load_builds_a_policy_for_the_running_process() {
        // Reads the real settings.json, so it asserts nothing about the outcome.
        let _ = PathPolicy::load();
        assert!(!PolicyDirs::current().temp.as_os_str().is_empty());
    }

    #[test]
    fn an_unusable_policy_refuses_every_path_and_says_why() {
        let sandbox = Sandbox::new();
        let file = sandbox.file_in(&sandbox.work, "a.txt");
        let policy = PathPolicy {
            allowed: vec![sandbox.work],
            protected: Vec::new(),
            unusable: Some("bad entry".to_string()),
        };

        for result in [policy.check_read(&file), policy.check_write(&file)] {
            let message = refusal(result);
            assert!(message.contains("unusable: bad entry"), "{message}");
        }
        assert!(policy.write(&file, b"x").is_err());
    }
}
