//! Helpers shared by the integration tests that spawn the `gwi` binary.

use std::path::Path;
use std::process::Command;

/// Variables the binary reads for credentials, accounts, profiles, endpoints,
/// logging, timeouts and leases. Tests remove these so their results do not
/// depend on the developer's shell (issues #62 and #120). `src/gmail/test_support.rs` and
/// `src/drive/test_support.rs` keep the in-process equivalent.
const AMBIENT_ENV: [&str; 44] = [
    "GWI_LOG_FILE",
    "GWI_LOG_DISABLE",
    "GWI_AUDIT_LOG_FILE",
    "GWI_LOG_MAX_SIZE",
    "GWI_LOG_KEEP_FILES",
    "GWI_LOG_BODIES",
    "GWI_LOG_HEADERS",
    "GWI_HTTP_CONNECT_TIMEOUT_SECS",
    "GWI_HTTP_READ_TIMEOUT_SECS",
    "GWI_SECRET_COMMAND_TIMEOUT_SECS",
    "GWI_SECRET_COMMAND_TTL_SECS",
    "GWI_DRIVE_LEASE_EXPIRY_MINUTES",
    "GWI_DRIVE_LEASE_BACKUP_DIR",
    "GWI_DRIVE_LEASE_BIOMETRICS_ONLY",
    "GWI_DRIVE_LEASE_ALLOW_HEADLESS",
    "GMAIL_CLIENT_ID",
    "GMAIL_CLIENT_SECRET",
    "GMAIL_CLIENT_SECRET_FILE",
    "GMAIL_REFRESH_TOKEN",
    "GMAIL_REFRESH_TOKEN_FILE",
    "GMAIL_REFRESH_TOKEN_COMMAND",
    "GMAIL_SCOPE",
    "GMAIL_API_URL",
    "GWI_GMAIL_ACCOUNT",
    "DRIVE_CLIENT_ID",
    "DRIVE_CLIENT_SECRET",
    "DRIVE_CLIENT_SECRET_FILE",
    "DRIVE_CLIENT_SECRET_COMMAND",
    "DRIVE_REFRESH_TOKEN",
    "DRIVE_REFRESH_TOKEN_FILE",
    "DRIVE_REFRESH_TOKEN_COMMAND",
    "DRIVE_SCOPE",
    "DRIVE_API_URL",
    "SHEETS_API_URL",
    "DOCS_API_URL",
    "SLIDES_API_URL",
    "GWI_DRIVE_ACCOUNT",
    "GWI_PROFILE",
    "GWI_CONFIG_DIR",
    "GWI_HOME",
    "GWI_STATE_DIR",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_CONFIG_HOME",
];

/// Removes every variable in [`AMBIENT_ENV`] from `command`'s environment.
///
/// Call it before the test's own `.env(..)` settings: `env_remove` after a
/// `.env` of the same name would undo it.
pub fn scrub_ambient_env(command: &mut Command) -> &mut Command {
    for name in AMBIENT_ENV {
        command.env_remove(name);
    }
    command
}

/// Pins both log sinks to `home` and disables request logging.
///
/// Audit logging remains enabled, so its path must be isolated too. Call this
/// before any test-specific log overrides.
pub fn pin_log_env<'a>(command: &'a mut Command, home: &Path) -> &'a mut Command {
    command
        .env("GWI_LOG_FILE", home.join("log.jsonl"))
        .env("GWI_AUDIT_LOG_FILE", home.join("audit.jsonl"))
        .env("GWI_LOG_DISABLE", "1")
}

/// Isolates production settings, state, credentials and both log sinks on every OS.
pub fn isolate<'a>(command: &'a mut Command, home: &Path) -> &'a mut Command {
    scrub_ambient_env(command)
        .env("HOME", home)
        .env("GWI_HOME", home)
        .env("GWI_STATE_DIR", home.join("state"));
    pin_log_env(command, home)
}
