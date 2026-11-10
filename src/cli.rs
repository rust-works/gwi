//! Command-line interface: the root command and the command trees under it.
//!
//! Ported from omni-dev's `cli` module (rust-works/omni-dev#2203) with only the
//! parts a Google Workspace tool needs: the global `--profile` flag, validation
//! of the selected profile before dispatch, and the command trees.

use anyhow::Result;
use clap::{Parser, Subcommand};

pub mod confirm;
pub mod drive;
pub mod format;
pub mod gmail;
pub mod import;
pub mod log;

/// Google Workspace Interface: Gmail, Drive, Docs, Sheets and Slides from the command line.
#[derive(Parser)]
#[command(name = "gwi")]
#[command(
    about = "Google Workspace Interface: Gmail, Drive, Docs, Sheets and Slides from the command line."
)]
#[command(version = crate::VERSION)]
pub struct Cli {
    /// Selects a named credential/config profile from
    /// `~/.gwi/settings.json` (AWS-CLI style).
    ///
    /// When set, the profile's `env` bundle replaces the base `env` map in the
    /// settings-fallback chain (process env still wins); the base map is not
    /// consulted. Overrides `GWI_PROFILE`. An unknown name is a hard error
    /// listing the known profiles.
    #[arg(
        help = "Selects a named credential/config profile from ~/.gwi/settings.json (AWS-CLI style)",
        long_help = "Selects a named credential/config profile from ~/.gwi/settings.json (AWS-CLI style).\n\nWhen set, the profile's env bundle replaces the base env map in the settings-fallback chain (process env still wins); the base map is not consulted. Overrides GWI_PROFILE. An unknown name is a hard error listing the known profiles."
    )]
    #[arg(long, global = true, value_name = "NAME")]
    pub profile: Option<String>,

    /// The main command to execute.
    #[command(subcommand)]
    pub command: Commands,
}

/// Top-level subcommand dispatch enum.
///
/// Each variant wraps the subcommand-specific argument struct; follow the
/// variant's payload type for the per-command argument surface.
#[derive(Subcommand)]
pub enum Commands {
    /// Gmail: search, read, and label messages via OAuth2.
    // Boxed: its subcommand tree is far larger than the other variants
    // (`clippy::large_enum_variant`). A plain comment, since a doc comment here
    // would become part of the command's `--help`.
    Gmail(Box<gmail::GmailCommand>),
    /// Drive: search, read, edit and sync Google Drive files, plus Docs, Sheets and Slides.
    Drive(Box<drive::DriveCommand>),
    /// Import: copy Gmail and Drive settings and the Drive lease ledger from omni-dev.
    Import(import::ImportCommand),
    /// Log: search the request and audit logs, and prune the request log.
    // Boxed: its flag set makes it the largest variant (`clippy::large_enum_variant`).
    Log(Box<log::LogCommand>),
}

impl Cli {
    /// Forwards `--profile` to the env var the settings readers discover the
    /// active profile from. Extracted so it can be unit-tested without
    /// invoking a real subcommand.
    fn propagate_profile_flag(&self) {
        // The flag beats the env var: setting `GWI_PROFILE` here means the
        // settings readers (which discover the active profile from that env
        // var) pick up the flag. When the flag is absent any existing
        // `GWI_PROFILE` is left untouched, so the env-var path still works.
        if let Some(profile) = &self.profile {
            std::env::set_var(crate::utils::settings::PROFILE_ENV_VAR, profile);
        }
    }

    /// Validates the active profile (resolved from `env`) against the settings
    /// produced by `load_settings`. The loader is invoked only when a profile is
    /// actually active, so a no-profile invocation reads no disk. Pure over its
    /// inputs, so it is unit-tested with a `MapEnv` and a constructed `Settings`
    /// rather than the process environment and `~/.gwi/settings.json`.
    fn validate_active_profile<E, F>(env: &E, load_settings: F) -> Result<()>
    where
        E: crate::utils::env::EnvSource,
        F: FnOnce() -> crate::utils::settings::Settings,
    {
        match crate::utils::settings::active_profile_from(env) {
            Some(name) => load_settings().validate_profile(&name),
            None => Ok(()),
        }
    }

    /// Thin disk boundary for [`Self::validate_active_profile`]: loads
    /// `~/.gwi/settings.json`, degrading to defaults when it is absent
    /// (silently) or unreadable/unparseable (with a warning) rather than
    /// failing, so an unreadable settings file cannot block commands that use no
    /// profile.
    fn load_settings_or_default() -> crate::utils::settings::Settings {
        crate::utils::settings::Settings::load_or_warn_default()
    }

    /// Executes the CLI command.
    pub async fn execute(self) -> Result<()> {
        self.propagate_profile_flag();

        // Validate the selected profile once, before dispatch, so a typo fails
        // fast rather than silently falling back to base credentials. The loader
        // runs only when a profile is active, so a no-profile invocation pays no
        // extra disk I/O.
        Self::validate_active_profile(
            &crate::utils::env::SystemEnv,
            Self::load_settings_or_default,
        )?;

        match self.command {
            Commands::Gmail(cmd) => (*cmd).execute().await,
            Commands::Drive(cmd) => (*cmd).execute().await,
            Commands::Import(cmd) => cmd.execute(),
            Commands::Log(cmd) => cmd.execute(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::test_support::env::MapEnv;
    use crate::utils::settings::Settings;

    // `execute()`'s command dispatch is otherwise only exercised by spawning
    // the real binary; this covers the `Gmail` arm in-process (deterministic,
    // network-free: missing credentials fail fast before any Gmail API call).
    #[tokio::test]
    async fn execute_routes_gmail_subcommand() {
        let guard = crate::gmail::test_support::EnvGuard::take();
        let _dir = guard.clear_credentials();

        let cli = Cli::try_parse_from(["gwi", "gmail", "auth", "status"]).unwrap();
        let err = cli.execute().await.unwrap_err();
        assert!(err.to_string().contains("not configured"));
    }

    #[test]
    fn profile_is_accepted_before_and_after_the_subcommand() {
        let before =
            Cli::try_parse_from(["gwi", "--profile", "work", "gmail", "auth", "status"]).unwrap();
        assert_eq!(before.profile.as_deref(), Some("work"));

        let after =
            Cli::try_parse_from(["gwi", "gmail", "auth", "status", "--profile", "work"]).unwrap();
        assert_eq!(after.profile.as_deref(), Some("work"));

        let none = Cli::try_parse_from(["gwi", "gmail", "auth", "status"]).unwrap();
        assert_eq!(none.profile, None);
    }

    #[test]
    fn a_missing_subcommand_is_a_usage_error() {
        assert!(Cli::try_parse_from(["gwi"]).is_err());
    }

    #[test]
    fn no_active_profile_never_loads_settings() {
        let env = MapEnv::new();
        let result = Cli::validate_active_profile(&env, || {
            panic!("settings must not be read when no profile is active")
        });
        assert!(result.is_ok());
    }

    #[test]
    fn a_known_profile_validates() {
        let env = MapEnv::new().with("GWI_PROFILE", "work");
        let settings: Settings =
            serde_json::from_str(r#"{"profiles":{"work":{"env":{}}}}"#).unwrap();
        assert!(Cli::validate_active_profile(&env, || settings).is_ok());
    }

    #[test]
    fn an_unknown_profile_fails_and_lists_the_known_ones() {
        let env = MapEnv::new().with("GWI_PROFILE", "wrok");
        let settings: Settings =
            serde_json::from_str(r#"{"profiles":{"work":{"env":{}},"home":{"env":{}}}}"#).unwrap();
        let err = Cli::validate_active_profile(&env, || settings).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("wrok"), "{message}");
        assert!(
            message.contains("home") && message.contains("work"),
            "{message}"
        );
    }
}
