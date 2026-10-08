//! Shared test helpers for Gmail unit tests.
//!
//! Any test that mutates `HOME` or a `GMAIL_*` environment variable must
//! acquire [`EnvGuard`] so parallel tests don't race on process-wide state.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::gmail::account::GMAIL_ACCOUNT_ENV;
use crate::gmail::auth::{
    GMAIL_API_URL, GMAIL_CLIENT_ID, GMAIL_CLIENT_SECRET, GMAIL_REFRESH_TOKEN, GMAIL_SCOPE,
};
use crate::utils::settings::PROFILE_ENV_VAR;

/// Process-wide mutex serialising tests that mutate `HOME` and the Gmail
/// credential environment variables.
///
/// Aliases the crate-wide [`crate::test_support::HOME_ENV_MUTEX`] so Gmail's
/// `HOME` mutation also serialises against every other domain's (Atlassian,
/// Datadog, …) — an independent `Mutex<()>` here provides no real exclusion
/// against them, which is exactly the race that surfaced between Gmail and
/// Datadog in issue #1465.
static GMAIL_ENV_MUTEX: &Mutex<()> = &crate::test_support::HOME_ENV_MUTEX;

/// RAII guard: snapshots `HOME`, `GWI_PROFILE` + every Gmail credential and
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
            GMAIL_CLIENT_ID.to_string(),
            GMAIL_CLIENT_SECRET.to_string(),
            GMAIL_REFRESH_TOKEN.to_string(),
            GMAIL_SCOPE.to_string(),
            GMAIL_ACCOUNT_ENV.to_string(),
            GMAIL_API_URL.to_string(),
        ];
        // The `_FILE` / `_COMMAND` companions of every Gmail secret, derived from
        // the registry so a new secret is covered without touching this list:
        // a developer who exports one changes the outcome of any test that
        // resolves credentials.
        keys.extend(
            crate::utils::secret_env::companion_vars()
                .into_iter()
                .filter(|var| var.starts_with("GMAIL_")),
        );
        // `gmail auth import`'s input, not a companion of a registered secret.
        keys.push(crate::gmail::import::GMAIL_CLIENT_SECRET_FILE.to_string());
        keys
    }

    pub(crate) fn take() -> Self {
        let lock = GMAIL_ENV_MUTEX
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
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
    /// `GMAIL_*` env vars.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_restores_the_profile_and_endpoint_variables_it_clears() {
        // `GWI_PROFILE` and `GMAIL_API_URL` were cleared by
        // `clear_credentials` but never snapshotted, so the first test to
        // call it erased an ambient value for every later test (#62).
        let read = || -> Vec<Option<String>> {
            [PROFILE_ENV_VAR, GMAIL_API_URL]
                .into_iter()
                .map(|key| std::env::var(key).ok())
                .collect()
        };
        let before = {
            let guard = EnvGuard::take();
            let before = read();
            guard.clear_credentials();
            std::env::set_var(PROFILE_ENV_VAR, "leaked-profile");
            std::env::set_var(GMAIL_API_URL, "http://127.0.0.1:1");
            before
        };
        let _guard = EnvGuard::take();
        assert_eq!(read(), before);
    }

    #[test]
    fn clear_credentials_clears_every_variable_the_guard_snapshots() {
        // Includes the `_FILE` / `_COMMAND` companions of each secret, which a
        // developer's shell can export.
        let guard = EnvGuard::take();
        let keys = EnvGuard::keys();
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
            .filter(|var| var.starts_with("GMAIL_"))
        {
            assert!(keys.contains(&companion), "{companion}");
        }
    }
}
