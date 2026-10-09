//! Shared test helpers for Drive unit tests.
//!
//! Any test that mutates `HOME` or a `DRIVE_*` environment variable must
//! acquire [`EnvGuard`] so parallel tests don't race on process-wide state.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::drive::account::DRIVE_ACCOUNT_ENV;
use crate::drive::auth::{
    DRIVE_API_URL, DRIVE_CLIENT_ID, DRIVE_CLIENT_SECRET, DRIVE_REFRESH_TOKEN, DRIVE_SCOPE,
};
use crate::drive::docs::client::DOCS_API_URL;
use crate::drive::sheets::client::SHEETS_API_URL;
use crate::drive::slides::client::SLIDES_API_URL;
use crate::utils::settings::PROFILE_ENV_VAR;

/// Process-wide mutex serialising tests that mutate `HOME` and the Drive
/// credential environment variables.
///
/// Aliases the crate-wide [`crate::test_support::HOME_ENV_MUTEX`] so Drive's
/// `HOME` mutation also serialises against every other domain's (Gmail,
/// Atlassian, Datadog, …) — an independent `Mutex<()>` here provides no real
/// exclusion against them, which is exactly the race that surfaced between
/// Gmail and Datadog in issue #1465.
static DRIVE_ENV_MUTEX: &Mutex<()> = &crate::test_support::HOME_ENV_MUTEX;

/// RAII guard: snapshots `HOME`, `GWI_PROFILE` + every Drive credential and
/// endpoint env var on construction and restores them on drop.
pub(crate) struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    snapshot: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    /// Every variable this guard snapshots, restores and clears.
    pub(crate) fn keys() -> Vec<String> {
        let mut keys = vec![
            "HOME".to_string(),
            PROFILE_ENV_VAR.to_string(),
            DRIVE_CLIENT_ID.to_string(),
            DRIVE_CLIENT_SECRET.to_string(),
            DRIVE_REFRESH_TOKEN.to_string(),
            DRIVE_SCOPE.to_string(),
            DRIVE_ACCOUNT_ENV.to_string(),
            DRIVE_API_URL.to_string(),
            SHEETS_API_URL.to_string(),
            DOCS_API_URL.to_string(),
            SLIDES_API_URL.to_string(),
        ];
        // The `_FILE` / `_COMMAND` companions of every Drive secret, derived from
        // the registry so a new secret is covered without touching this list:
        // a developer who exports one changes the outcome of any test that
        // resolves credentials.
        keys.extend(
            crate::utils::secret_env::companion_vars()
                .into_iter()
                .filter(|var| var.starts_with("DRIVE_")),
        );
        keys
    }

    pub(crate) fn take() -> Self {
        let lock = DRIVE_ENV_MUTEX
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // The four `*_API_URL` host overrides are snapshotted for two
        // reasons: a developer with one exported must not silently redirect
        // a test's requests, and a test that points one at a local server
        // must not leak that setting into the next test. Their absence here
        // was already a latent hazard for `DRIVE_API_URL`; `SHEETS_API_URL`
        // and `DOCS_API_URL` make it sharper, since without an override
        // those clients default to the *real* `sheets.googleapis.com` /
        // `docs.googleapis.com`.
        let snapshot = Self::keys()
            .into_iter()
            .map(|k| {
                let value = std::env::var(&k).ok();
                (k, value)
            })
            .collect();
        Self {
            _lock: lock,
            snapshot,
        }
    }

    /// Sets `HOME` to a fresh tempdir and clears `GWI_PROFILE` and all
    /// `DRIVE_*` env vars.
    ///
    /// Returns the tempdir so the caller can inspect the
    /// `.gwi/settings.json` written inside it.
    pub(crate) fn clear_credentials(&self) -> tempfile::TempDir {
        let dir = {
            std::fs::create_dir_all("tmp").ok();
            tempfile::TempDir::new_in("tmp").unwrap()
        };
        for key in Self::keys() {
            std::env::remove_var(key);
        }
        std::env::set_var("HOME", dir.path());
        dir
    }

    /// Points every Google API host override at a dead local address.
    ///
    /// For tests that exercise a code path which *constructs* a client
    /// without a wiremock server in hand. Without this a `SheetsClient` or
    /// `DocsClient` falls back to the real `sheets.googleapis.com` /
    /// `docs.googleapis.com`, so a routing test would make an outbound
    /// request to Google.
    ///
    /// Every new second-host client must be added here as well as to
    /// [`Self::take`]'s snapshot — this is the one of the two whose omission
    /// is silent, because the test still passes while talking to Google.
    pub(crate) fn redirect_api_hosts_to_a_dead_port(&self) {
        std::env::set_var(DRIVE_API_URL, "http://127.0.0.1:1");
        std::env::set_var(SHEETS_API_URL, "http://127.0.0.1:1");
        std::env::set_var(DOCS_API_URL, "http://127.0.0.1:1");
        std::env::set_var(SLIDES_API_URL, "http://127.0.0.1:1");
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, v) in &self.snapshot {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
    }
}

// ── Lease ledger fixtures ────────────────────────────────────────────────

use crate::drive::lease::ledger::{LeaseBackup, LeaseLedger, LeaseRecord};

/// The dummy backup every `seed_lease*` variant uses when the caller
/// doesn't need to control it.
fn dummy_lease_backup() -> LeaseBackup {
    LeaseBackup::Bytes {
        path: std::path::PathBuf::from("/tmp/test-backup"),
        sha256: "deadbeef".to_string(),
        size: 0,
        extra: serde_json::Map::new(),
    }
}

/// Seeds `ledger_path` with a fresh, live lease for `file_id` at `version`
/// using the fixed token `"test-lease-token"`, a dummy `Bytes` backup, and
/// a 30-minute expiry, returning the token — the shape every lease-gated
/// write engine's tests need.
pub(crate) fn seed_lease(ledger_path: &std::path::Path, file_id: &str, version: &str) -> String {
    seed_lease_with_backup(ledger_path, file_id, version, dummy_lease_backup())
}

/// [`seed_lease`], with an explicit `backup` instead of the dummy one — for
/// tests that assert on the backup's own content/shape.
pub(crate) fn seed_lease_with_backup(
    ledger_path: &std::path::Path,
    file_id: &str,
    version: &str,
    backup: LeaseBackup,
) -> String {
    seed_lease_full(
        ledger_path,
        "test-lease-token",
        file_id,
        version,
        chrono::Duration::minutes(30),
        backup,
    )
}

/// [`seed_lease_full`], but with explicit `acquired_at`/`expires_at`
/// timestamps instead of a relative expiry — for tests that need to
/// control staleness independent of "now" (e.g. a lease already expired,
/// or held for a specific duration).
pub(crate) fn seed_lease_at(
    ledger_path: &std::path::Path,
    token: &str,
    file_id: &str,
    version: &str,
    acquired_at: chrono::DateTime<chrono::Utc>,
    expires_at: chrono::DateTime<chrono::Utc>,
    backup: LeaseBackup,
) -> String {
    let mut ledger = LeaseLedger::default();
    ledger.insert(LeaseRecord {
        token: token.to_string(),
        file_id: file_id.to_string(),
        version: version.to_string(),
        modified_time: None,
        backup,
        acquired_at,
        expires_at,
        released_at: None,
        superseded_by: None,
        restored_at: None,
        restored_sheet_id: None,
        extra: serde_json::Map::new(),
    });
    ledger.save(ledger_path).unwrap();
    token.to_string()
}

/// The fully general form: explicit token and expiry as well as backup.
pub(crate) fn seed_lease_full(
    ledger_path: &std::path::Path,
    token: &str,
    file_id: &str,
    version: &str,
    expiry: chrono::Duration,
    backup: LeaseBackup,
) -> String {
    let now = chrono::Utc::now();
    seed_lease_at(
        ledger_path,
        token,
        file_id,
        version,
        now,
        now + expiry,
        backup,
    )
}

// ── Authenticator test doubles ──────────────────────────────────────────

use crate::drive::lease::authenticate::{AuthOutcome, AuthPolicy, Authenticator};

/// An [`Authenticator`] that always returns a fixed, caller-supplied
/// [`AuthOutcome`] — for tests exercising both the authorized and
/// denied/unavailable paths without a real prompt.
pub(crate) struct FakeAuthenticator(pub(crate) AuthOutcome);

impl Authenticator for FakeAuthenticator {
    fn authenticate(&self, _reason: &str, _policy: AuthPolicy) -> AuthOutcome {
        self.0.clone()
    }
}

/// A read-only Drive client with OAuth refresh directed at the local mock server.
pub(crate) async fn client_with_bootstrapped_token(
    server: &wiremock::MockServer,
) -> crate::drive::client::DriveClient {
    use crate::drive::auth::{DriveCredentials, DriveGrantedScopes};
    use crate::drive::client::DriveClient;
    use crate::utils::secret::Secret;
    let credentials = DriveCredentials {
        client_id: "client".into(),
        client_secret: Secret::new("secret"),
        refresh_token: Secret::new("refresh"),
        scope: DriveGrantedScopes::READONLY,
    };
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/token"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"access_token":"token","expires_in":3600})),
        )
        .mount(server)
        .await;
    let mut client = DriveClient::new(&server.uri(), &credentials).unwrap();
    crate::drive::client::test_support::replace_session(
        &mut client,
        &credentials,
        &format!("{}/token", server.uri()),
    );
    client
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// The identifiers of every Drive and Gmail key the two `EnvGuard`s
    /// snapshot. `HOME` is left out: Atlassian's and other domains' own
    /// guards mutate it legitimately without these. `GWI_PROFILE` is left
    /// out too: `Cli::propagate_profile_flag` sets it in production code.
    const GUARDED_KEYS: &[&str] = &[
        "DRIVE_CLIENT_ID",
        "DRIVE_CLIENT_SECRET",
        "DRIVE_REFRESH_TOKEN",
        "DRIVE_SCOPE",
        "DRIVE_ACCOUNT_ENV",
        "DRIVE_API_URL",
        "SHEETS_API_URL",
        "DOCS_API_URL",
        "SLIDES_API_URL",
        "GMAIL_CLIENT_ID",
        "GMAIL_CLIENT_SECRET",
        "GMAIL_REFRESH_TOKEN",
        "GMAIL_SCOPE",
        "GMAIL_ACCOUNT_ENV",
        "GMAIL_API_URL",
    ];

    fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                rust_sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Grep guard: any function that sets or removes one of the Drive or
    /// Gmail keys must hold an `EnvGuard`, or it races every guarded test that
    /// points the same key at its own server — exactly how an unguarded
    /// `set_var(SHEETS_API_URL, …)` in a text-to-columns test broke
    /// `the_pivot_table_dispatch_arms_reach_their_leaf_commands` (#2035).
    ///
    /// Splitting on `fn ` is a heuristic: a nested `fn` ahead of the
    /// mutation would hide the `take()` above it and fail here, which is
    /// the loud direction.
    #[test]
    fn every_gmail_and_drive_env_mutation_holds_the_env_guard() {
        let mutation = regex::Regex::new(&format!(
            r#"(?:set_var|remove_var)\(\s*"?(?:[A-Za-z_][A-Za-z0-9_]*::)*(?:{})\b"#,
            GUARDED_KEYS.join("|")
        ))
        .unwrap();
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let this_file = src.join("drive").join("test_support.rs");
        let mut files = Vec::new();
        rust_sources(&src, &mut files);

        let mut offenders = Vec::new();
        for path in files.into_iter().filter(|p| *p != this_file) {
            let text = std::fs::read_to_string(&path).unwrap();
            for body in text.split("fn ").skip(1) {
                if mutation.is_match(body) && !body.contains("EnvGuard::take()") {
                    // patchcov: coverage ignore reason="only runs if a function mutates a Gmail or Drive env var without EnvGuard::take(); offenders.is_empty() below is this test's whole point"
                    let name = body.split(['(', '<']).next().unwrap_or_default();
                    offenders.push(format!(
                        "{}: fn {name}",
                        path.strip_prefix(&src).unwrap().display()
                    ));
                    // patchcov: coverage end
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "these functions mutate a Gmail or Drive env var without \
             `EnvGuard::take()`:\n{}",
            offenders.join("\n") // patchcov: coverage ignore-line reason="assert! message args only evaluate when the condition is false, i.e. an offender was found"
        );
    }

    #[test]
    fn drop_restores_the_profile_and_endpoint_variables_it_clears() {
        // `GWI_PROFILE` was cleared by `clear_credentials` but never
        // snapshotted, so the first test to call it erased an ambient value
        // for every later test (#62).
        use super::{EnvGuard, PROFILE_ENV_VAR};
        let read = || -> Vec<Option<String>> {
            [PROFILE_ENV_VAR, super::DRIVE_API_URL]
                .into_iter()
                .map(|key| std::env::var(key).ok())
                .collect()
        };
        let before = {
            let guard = EnvGuard::take();
            let before = read();
            guard.clear_credentials();
            std::env::set_var(PROFILE_ENV_VAR, "leaked-profile");
            std::env::set_var(super::DRIVE_API_URL, "http://127.0.0.1:1");
            before
        };
        let _guard = EnvGuard::take();
        assert_eq!(read(), before);
    }

    #[test]
    fn clear_credentials_clears_every_variable_the_guard_snapshots() {
        // Includes the `_FILE` / `_COMMAND` companions of each secret, which a
        // developer's shell can export.
        let guard = super::EnvGuard::take();
        let keys = super::EnvGuard::keys();
        // `HOME` stays put: tests that do not hold the mutex still log under it.
        for key in keys.iter().filter(|key| *key != "HOME") {
            std::env::set_var(key, "hostile");
        }
        let _home = guard.clear_credentials();
        for key in keys.iter().filter(|key| *key != "HOME") {
            assert_eq!(std::env::var(key).ok(), None, "{key}");
        }
        for companion in crate::utils::secret_env::companion_vars()
            .into_iter()
            .filter(|var| var.starts_with("DRIVE_"))
        {
            assert!(keys.contains(&companion), "{companion}");
        }
    }

    /// `tests/common/mod.rs` cannot call the `#[cfg(test)]` registry helper, so
    /// its list of variables to scrub from spawned binaries is written out; this
    /// keeps it covering everything the in-process guards snapshot.
    #[test]
    fn the_integration_test_scrub_list_covers_every_guarded_variable() {
        let list = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/common/mod.rs"),
        )
        .unwrap();
        let guarded = super::EnvGuard::keys()
            .into_iter()
            .chain(crate::gmail::test_support::EnvGuard::keys());
        for key in guarded.filter(|key| key != "HOME") {
            assert!(list.contains(&format!("\"{key}\"")), "{key}");
        }
    }
}
