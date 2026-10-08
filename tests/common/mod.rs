//! Helpers shared by the integration tests that spawn the `gwi` binary.

use std::process::Command;

/// The credential, account, profile and endpoint variables the binary reads
/// from its environment, so a test that must not depend on the developer's
/// shell removes them all (issue #62). `src/gmail/test_support.rs` and
/// `src/drive/test_support.rs` keep the in-process equivalent.
const AMBIENT_ENV: [&str; 27] = [
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
