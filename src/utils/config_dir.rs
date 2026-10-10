//! Config-directory discovery: where `.gwi/` lives and which file wins.
//!
//! Forked from omni-dev's `claude::context::discovery` (rust-works/omni-dev#2203);
//! only the directory and file resolution comes over, not omni-dev's commit-scope
//! and project-convention discovery.

use std::path::{Path, PathBuf};

use std::fmt;

use crate::utils::env::{EnvSource, SystemEnv};

/// Returns the XDG-compliant config directory for gwi.
///
/// Uses `$XDG_CONFIG_HOME/gwi/` if the variable is set, otherwise
/// defaults to `{home}/.config/gwi/` per the XDG Base Directory
/// Specification. Returns `None` if neither can be determined.
///
/// Reads `XDG_CONFIG_HOME` from the injected `env` rather than via
/// `dirs::config_dir()`, which returns `~/Library/Application Support/` on
/// macOS — not the expected location for a CLI tool. Taking `env`/`home` as
/// parameters keeps this resolver pure: production callers pass `&SystemEnv`
/// and the application home, while tests pass a `MapEnv` (the `#[cfg(test)]`
/// `crate::test_support::env::MapEnv`) and a temp `home` without mutating the
/// environment (STYLE-0028, issue #821).
fn xdg_config_dir_with(env: &impl EnvSource, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(xdg_home) = env.var("XDG_CONFIG_HOME") {
        if !xdg_home.is_empty() {
            return Some(PathBuf::from(xdg_home).join("gwi"));
        }
    }

    // Default: $HOME/.config/gwi/
    home.map(|home| home.join(".config").join("gwi"))
}

/// Resolves configuration file path with local override support and global fallback.
///
/// Priority:
/// 1. `{dir}/local/{filename}` (local override)
/// 2. `{dir}/{filename}` (shared project config)
/// 3. `$XDG_CONFIG_HOME/gwi/{filename}` (XDG global config)
/// 4. `$HOME/.gwi/{filename}` (legacy global fallback)
pub fn resolve_config_file(dir: &Path, filename: &str) -> PathBuf {
    resolve_config_file_with(
        dir,
        filename,
        &SystemEnv,
        crate::utils::app_dirs::home_dir().as_deref(),
    )
}

/// Inner seam for [`resolve_config_file`]: the XDG and legacy-home tiers read
/// from an injected `env`/`home` rather than process-global state, so tests
/// drive the full priority chain without mutating the environment
/// (STYLE-0028, issue #821).
fn resolve_config_file_with(
    dir: &Path,
    filename: &str,
    env: &impl EnvSource,
    home: Option<&Path>,
) -> PathBuf {
    let local_path = dir.join("local").join(filename);
    if local_path.exists() {
        return local_path;
    }

    let project_path = dir.join(filename);
    if project_path.exists() {
        return project_path;
    }

    // Check XDG config directory
    if let Some(xdg_dir) = xdg_config_dir_with(env, home) {
        let xdg_path = xdg_dir.join(filename);
        if xdg_path.exists() {
            return xdg_path;
        }
    }

    // Check legacy home directory fallback
    if let Some(home_dir) = home {
        let home_path = home_dir.join(".gwi").join(filename);
        if home_path.exists() {
            return home_path;
        }
    }

    // Return project path as default (even if it doesn't exist)
    project_path
}

/// Walks up from `start` toward the repository root, looking for `.gwi/`.
///
/// Returns the first `.gwi/` directory found. Stops at the repository
/// root (identified by a `.git` directory or file). Returns `None` if no
/// `.gwi/` is found within the repository boundary.
fn walk_up_find_config_dir(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        let candidate = current.join(".gwi");
        if candidate.is_dir() {
            return Some(candidate);
        }
        // Stop at repo root — don't escape the repository
        if current.join(".git").exists() {
            break;
        }
        if !current.pop() {
            break;
        }
    }
    None
}

/// Identifies how the context directory was resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigDirSource {
    /// Explicitly set via `--context-dir` CLI flag.
    CliFlag,
    /// Set via `GWI_CONFIG_DIR` environment variable.
    EnvVar,
    /// Found via walk-up discovery from CWD.
    WalkUp,
    /// Default `.gwi` (no explicit override, no walk-up match).
    Default,
}

impl fmt::Display for ConfigDirSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CliFlag => write!(f, "--context-dir"),
            Self::EnvVar => write!(f, "GWI_CONFIG_DIR"),
            Self::WalkUp => write!(f, "walk-up"),
            Self::Default => write!(f, "default"),
        }
    }
}

/// Resolves the context directory and reports how it was selected.
///
/// Priority:
/// 1. `override_dir` (from `--context-dir` CLI flag; disables walk-up)
/// 2. `GWI_CONFIG_DIR` environment variable (disables walk-up)
/// 3. Walk-up: nearest `.gwi/` from CWD to repo root
/// 4. `.gwi` default
pub fn resolve_context_dir_with_source(override_dir: Option<&Path>) -> (PathBuf, ConfigDirSource) {
    resolve_context_dir_with_source_env(override_dir, &SystemEnv)
}

/// Inner seam for [`resolve_context_dir_with_source`]: reads `GWI_CONFIG_DIR`
/// from an injected `env` rather than process-global state, so tests cover the
/// env-var tier without mutating the environment (STYLE-0028, issue #821). The
/// CWD walk-up tier still consults the (read-only) process working directory.
fn resolve_context_dir_with_source_env(
    override_dir: Option<&Path>,
    env: &impl EnvSource,
) -> (PathBuf, ConfigDirSource) {
    if let Some(dir) = override_dir {
        return (dir.to_path_buf(), ConfigDirSource::CliFlag);
    }

    if let Some(env_dir) = env.var("GWI_CONFIG_DIR") {
        if !env_dir.is_empty() {
            return (PathBuf::from(env_dir), ConfigDirSource::EnvVar);
        }
    }

    // Walk-up discovery: search from CWD upward to repo root
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(config_dir) = walk_up_find_config_dir(&cwd) {
            return (config_dir, ConfigDirSource::WalkUp);
        }
    }

    (PathBuf::from(".gwi"), ConfigDirSource::Default)
}
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::test_support::env::MapEnv;
    use tempfile::TempDir;

    // ── resolve_config_file ──────────────────────────────────────────

    #[test]
    fn local_override_wins() -> anyhow::Result<()> {
        let dir = {
            std::fs::create_dir_all("tmp")?;
            TempDir::new_in("tmp")?
        };
        let base = dir.path();

        // Create both local and project files
        std::fs::create_dir_all(base.join("local"))?;
        std::fs::write(base.join("local").join("scopes.yaml"), "local")?;
        std::fs::write(base.join("scopes.yaml"), "project")?;

        let resolved = resolve_config_file(base, "scopes.yaml");
        assert_eq!(resolved, base.join("local").join("scopes.yaml"));
        Ok(())
    }

    #[test]
    fn project_fallback() -> anyhow::Result<()> {
        let dir = {
            std::fs::create_dir_all("tmp")?;
            TempDir::new_in("tmp")?
        };
        let base = dir.path();

        // Create only project-level file (no local/)
        std::fs::write(base.join("scopes.yaml"), "project")?;

        let resolved = resolve_config_file(base, "scopes.yaml");
        assert_eq!(resolved, base.join("scopes.yaml"));
        Ok(())
    }

    #[test]
    fn returns_default_when_nothing_exists() {
        let dir = {
            std::fs::create_dir_all("tmp").ok();
            TempDir::new_in("tmp").unwrap()
        };
        let base = dir.path();

        let resolved = resolve_config_file(base, "scopes.yaml");
        // When no local or project file exists, it either returns:
        // - the home directory path if $HOME/.gwi/scopes.yaml exists
        // - the project path as fallback default
        // Either way, the resolved path should NOT be the local override path.
        assert_ne!(resolved, base.join("local").join("scopes.yaml"));
    }

    // ── resolve_context_dir ────────────────────────────────────────────

    // These tests inject a `MapEnv` into the `*_env` seam instead of mutating
    // `GWI_CONFIG_DIR`, so they need no lock and run fully in parallel
    // (STYLE-0028, issue #821).

    #[test]
    fn context_dir_defaults_to_gwi() {
        let result = resolve_context_dir_with_source_env(None, &MapEnv::new()).0;
        // Walk-up may find .gwi in the real repo, or fall back to ".gwi"
        assert!(
            result.ends_with(".gwi"),
            "expected path ending in .gwi, got {result:?}"
        );
    }

    #[test]
    fn context_dir_uses_override() {
        let custom = PathBuf::from("custom-config");
        let result = resolve_context_dir_with_source_env(Some(&custom), &MapEnv::new()).0;
        assert_eq!(result, custom);
    }

    #[test]
    fn context_dir_env_var() {
        let env = MapEnv::new().with("GWI_CONFIG_DIR", "/tmp/my-config");
        let result = resolve_context_dir_with_source_env(None, &env).0;
        assert_eq!(result, PathBuf::from("/tmp/my-config"));
    }

    #[test]
    fn context_dir_cli_flag_beats_env_var() {
        let env = MapEnv::new().with("GWI_CONFIG_DIR", "/tmp/env-config");
        let cli = PathBuf::from("cli-config");
        let result = resolve_context_dir_with_source_env(Some(&cli), &env).0;
        assert_eq!(result, cli);
    }

    #[test]
    fn context_dir_ignores_empty_env_var() {
        let env = MapEnv::new().with("GWI_CONFIG_DIR", "");
        let result = resolve_context_dir_with_source_env(None, &env).0;
        // Walk-up may find .gwi in the real repo, or fall back to ".gwi"
        assert!(
            result.ends_with(".gwi"),
            "expected path ending in .gwi, got {result:?}"
        );
    }

    // ── resolve_context_dir_with_source ─────────────────────────────────

    #[test]
    fn with_source_cli_flag() {
        let custom = PathBuf::from("custom-config");
        let (path, source) = resolve_context_dir_with_source_env(Some(&custom), &MapEnv::new());
        assert_eq!(path, custom);
        assert_eq!(source, ConfigDirSource::CliFlag);
    }

    #[test]
    fn with_source_env_var() {
        let env = MapEnv::new().with("GWI_CONFIG_DIR", "/tmp/env-config");
        let (path, source) = resolve_context_dir_with_source_env(None, &env);
        assert_eq!(path, PathBuf::from("/tmp/env-config"));
        assert_eq!(source, ConfigDirSource::EnvVar);
    }

    #[test]
    fn with_source_cli_beats_env() {
        let env = MapEnv::new().with("GWI_CONFIG_DIR", "/tmp/env-config");
        let custom = PathBuf::from("cli-config");
        let (path, source) = resolve_context_dir_with_source_env(Some(&custom), &env);
        assert_eq!(path, custom);
        assert_eq!(source, ConfigDirSource::CliFlag);
    }

    #[test]
    fn with_source_walk_up_or_default() {
        let (path, source) = resolve_context_dir_with_source_env(None, &MapEnv::new());
        // Inside this repo, walk-up finds .gwi; outside, falls back to default
        assert!(
            path.ends_with(".gwi"),
            "expected path ending in .gwi, got {path:?}"
        );
        assert!(
            source == ConfigDirSource::WalkUp || source == ConfigDirSource::Default,
            "expected WalkUp or Default, got {source:?}"
        );
    }

    // ── ConfigDirSource Display ──────────────────────────────────────────

    #[test]
    fn display_config_dir_source_cli_flag() {
        assert_eq!(ConfigDirSource::CliFlag.to_string(), "--context-dir");
    }

    #[test]
    fn display_config_dir_source_env_var() {
        assert_eq!(ConfigDirSource::EnvVar.to_string(), "GWI_CONFIG_DIR");
    }

    #[test]
    fn display_config_dir_source_walk_up() {
        assert_eq!(ConfigDirSource::WalkUp.to_string(), "walk-up");
    }

    #[test]
    fn display_config_dir_source_default() {
        assert_eq!(ConfigDirSource::Default.to_string(), "default");
    }

    // ── xdg_config_dir ─────────────────────────────────────────────────

    #[test]
    fn xdg_config_dir_uses_env_var() {
        let env = MapEnv::new().with("XDG_CONFIG_HOME", "/tmp/xdg-test");
        let result = xdg_config_dir_with(&env, None);
        assert_eq!(result, Some(PathBuf::from("/tmp/xdg-test/gwi")));
    }

    #[test]
    fn xdg_config_dir_ignores_empty_env_var() {
        // An empty `XDG_CONFIG_HOME` falls back to `$HOME/.config/gwi`.
        let env = MapEnv::new().with("XDG_CONFIG_HOME", "");
        let home = Path::new("/test-home");
        let result = xdg_config_dir_with(&env, Some(home));
        assert_eq!(result, Some(home.join(".config").join("gwi")));
    }

    #[test]
    fn xdg_config_dir_defaults_to_home_config() {
        // With `XDG_CONFIG_HOME` unset, the injected home base is used.
        let home = Path::new("/test-home");
        let result = xdg_config_dir_with(&MapEnv::new(), Some(home));
        assert_eq!(result, Some(home.join(".config").join("gwi")));
    }

    // ── resolve_config_file XDG integration ─────────────────────────────

    /// Builds a `MapEnv` pointing `XDG_CONFIG_HOME` at `path`, for the
    /// `resolve_config_file_with` / `config_source_label_with` seams.
    fn xdg_env(path: &Path) -> MapEnv {
        MapEnv::new().with(
            "XDG_CONFIG_HOME",
            path.to_str().expect("temp path is valid UTF-8"),
        )
    }

    #[test]
    fn resolve_config_file_finds_xdg() -> anyhow::Result<()> {
        let xdg_dir = {
            std::fs::create_dir_all("tmp")?;
            TempDir::new_in("tmp")?
        };
        let xdg_omni = xdg_dir.path().join("gwi");
        std::fs::create_dir_all(&xdg_omni)?;
        std::fs::write(xdg_omni.join("commit-guidelines.md"), "xdg content")?;

        let project_dir = {
            std::fs::create_dir_all("tmp")?;
            TempDir::new_in("tmp")?
        };
        let resolved = resolve_config_file_with(
            project_dir.path(),
            "commit-guidelines.md",
            &xdg_env(xdg_dir.path()),
            None,
        );

        assert_eq!(resolved, xdg_omni.join("commit-guidelines.md"));
        Ok(())
    }

    #[test]
    fn resolve_config_file_xdg_beats_home() -> anyhow::Result<()> {
        // Set up XDG config
        let xdg_dir = {
            std::fs::create_dir_all("tmp")?;
            TempDir::new_in("tmp")?
        };
        let xdg_omni = xdg_dir.path().join("gwi");
        std::fs::create_dir_all(&xdg_omni)?;
        std::fs::write(xdg_omni.join("scopes.yaml"), "xdg")?;

        // Project dir with no local config
        let project_dir = {
            std::fs::create_dir_all("tmp")?;
            TempDir::new_in("tmp")?
        };

        let resolved = resolve_config_file_with(
            project_dir.path(),
            "scopes.yaml",
            &xdg_env(xdg_dir.path()),
            None,
        );

        // XDG path should win (home path only wins if XDG doesn't have the file)
        assert_eq!(resolved, xdg_omni.join("scopes.yaml"));
        Ok(())
    }

    #[test]
    fn resolve_config_file_project_beats_xdg() -> anyhow::Result<()> {
        // Set up XDG config
        let xdg_dir = {
            std::fs::create_dir_all("tmp")?;
            TempDir::new_in("tmp")?
        };
        let xdg_omni = xdg_dir.path().join("gwi");
        std::fs::create_dir_all(&xdg_omni)?;
        std::fs::write(xdg_omni.join("scopes.yaml"), "xdg")?;

        // Project dir with project-level config
        let project_dir = {
            std::fs::create_dir_all("tmp")?;
            TempDir::new_in("tmp")?
        };
        std::fs::write(project_dir.path().join("scopes.yaml"), "project")?;

        let resolved = resolve_config_file_with(
            project_dir.path(),
            "scopes.yaml",
            &xdg_env(xdg_dir.path()),
            None,
        );

        // Project path should win over XDG
        assert_eq!(resolved, project_dir.path().join("scopes.yaml"));
        Ok(())
    }

    // ── walk_up_find_config_dir ─────────────────────────────────────────

    /// Creates a mock repo tree with `.git` at the root.
    /// Returns (root_dir, TempDir handle).
    fn make_repo_tree() -> anyhow::Result<TempDir> {
        let dir = {
            std::fs::create_dir_all("tmp")?;
            TempDir::new_in("tmp")?
        };
        // Create .git marker at root
        std::fs::create_dir(dir.path().join(".git"))?;
        Ok(dir)
    }

    #[test]
    fn walk_up_finds_gwi_in_start_dir() -> anyhow::Result<()> {
        let repo = make_repo_tree()?;
        let sub = repo.path().join("packages").join("frontend");
        std::fs::create_dir_all(&sub)?;
        std::fs::create_dir(sub.join(".gwi"))?;

        let result = walk_up_find_config_dir(&sub);
        assert_eq!(result, Some(sub.join(".gwi")));
        Ok(())
    }

    #[test]
    fn walk_up_finds_gwi_in_parent() -> anyhow::Result<()> {
        let repo = make_repo_tree()?;
        let pkg = repo.path().join("packages").join("frontend");
        let src = pkg.join("src");
        std::fs::create_dir_all(&src)?;
        std::fs::create_dir(pkg.join(".gwi"))?;

        let result = walk_up_find_config_dir(&src);
        assert_eq!(result, Some(pkg.join(".gwi")));
        Ok(())
    }

    #[test]
    fn walk_up_finds_gwi_at_repo_root() -> anyhow::Result<()> {
        let repo = make_repo_tree()?;
        let deep = repo.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&deep)?;
        std::fs::create_dir(repo.path().join(".gwi"))?;

        let result = walk_up_find_config_dir(&deep);
        assert_eq!(result, Some(repo.path().join(".gwi")));
        Ok(())
    }

    #[test]
    fn walk_up_nearest_wins() -> anyhow::Result<()> {
        let repo = make_repo_tree()?;
        let pkg = repo.path().join("packages").join("frontend");
        let src = pkg.join("src");
        std::fs::create_dir_all(&src)?;
        // Both root and package have .gwi
        std::fs::create_dir(repo.path().join(".gwi"))?;
        std::fs::create_dir(pkg.join(".gwi"))?;

        let result = walk_up_find_config_dir(&src);
        // Nearest (packages/frontend/.gwi) should win
        assert_eq!(result, Some(pkg.join(".gwi")));
        Ok(())
    }

    #[test]
    fn walk_up_stops_at_git_boundary() -> anyhow::Result<()> {
        let dir = {
            std::fs::create_dir_all("tmp")?;
            TempDir::new_in("tmp")?
        };
        // Parent has .gwi but is outside the repo
        std::fs::create_dir(dir.path().join(".gwi"))?;
        // Repo root is a subdirectory
        let repo_root = dir.path().join("repo");
        std::fs::create_dir_all(&repo_root)?;
        std::fs::create_dir(repo_root.join(".git"))?;
        let sub = repo_root.join("sub");
        std::fs::create_dir(&sub)?;

        let result = walk_up_find_config_dir(&sub);
        // Should NOT find the .gwi above .git
        assert_eq!(result, None);
        Ok(())
    }

    #[test]
    fn walk_up_returns_none_when_no_gwi() -> anyhow::Result<()> {
        let repo = make_repo_tree()?;
        let sub = repo.path().join("src");
        std::fs::create_dir(&sub)?;

        let result = walk_up_find_config_dir(&sub);
        assert_eq!(result, None);
        Ok(())
    }

    #[test]
    fn walk_up_handles_git_worktree_file() -> anyhow::Result<()> {
        let dir = {
            std::fs::create_dir_all("tmp")?;
            TempDir::new_in("tmp")?
        };
        // .git as a file (worktree)
        std::fs::write(dir.path().join(".git"), "gitdir: /some/path")?;
        std::fs::create_dir(dir.path().join(".gwi"))?;
        let sub = dir.path().join("src");
        std::fs::create_dir(&sub)?;

        let result = walk_up_find_config_dir(&sub);
        assert_eq!(result, Some(dir.path().join(".gwi")));
        Ok(())
    }

    #[test]
    fn walk_up_no_gwi_in_repo_returns_none() -> anyhow::Result<()> {
        // Repo with .git but no .gwi anywhere
        let repo = make_repo_tree()?;
        let sub = repo.path().join("a").join("b");
        std::fs::create_dir_all(&sub)?;
        let result = walk_up_find_config_dir(&sub);
        assert_eq!(result, None);
        Ok(())
    }
}
