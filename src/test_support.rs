//! Shared test-only helpers.
//!
//! These utilities are consumed by unit tests across the crate and must
//! stay in sync between shim-writing sites — see issue #642.

#![allow(clippy::unwrap_used, clippy::expect_used)]

/// Process-wide mutex serialising every test in the crate that mutates the
/// global `HOME` environment variable, or any credential env var whose
/// resolution depends on it (`dirs::home_dir()` /
/// `Settings::get_settings_path()`).
///
/// Every HOME-mutating test-support module (Atlassian, Datadog, Gmail, the
/// `ai_chat`/`cli::ai::chat` provider tests, …) aliases this **one** static
/// rather than declaring its own `Mutex<()>`. Independent per-module mutexes
/// provide no mutual exclusion against each other — each only serialises its
/// own module's tests — while `HOME` itself is shared process-wide state, so
/// two modules' tests can still interleave and race on it. That exact
/// pattern caused the flaky race fixed for Atlassian in issue #950, and
/// resurfaced as a Gmail-vs-Datadog race (both mutating `HOME` under their
/// own independent mutex) in issue #1465.
pub(crate) static HOME_ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Captures native environment values and restores them on drop.
/// The caller must hold `HOME_ENV_MUTEX` until after this snapshot is dropped.
pub(crate) struct EnvSnapshot {
    values: Vec<(String, Option<std::ffi::OsString>)>,
}

impl EnvSnapshot {
    pub(crate) fn take(
        keys: impl IntoIterator<Item = String>,
        _lock: &std::sync::MutexGuard<'_, ()>,
    ) -> Self {
        Self {
            values: keys
                .into_iter()
                .map(|key| {
                    let value = std::env::var_os(&key);
                    (key, value)
                })
                .collect(),
        }
    }
}

impl Drop for EnvSnapshot {
    fn drop(&mut self) {
        for (key, value) in &self.values {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

thread_local! {
    static SETTINGS_PATH: std::cell::RefCell<Option<std::path::PathBuf>> = const {
        std::cell::RefCell::new(None)
    };
}

/// An explicit settings fixture for the current test thread. Windows' Known
/// Folder API ignores `HOME`, so changing that variable cannot isolate settings.
/// Like the audit-log route, this affects unit tests only, not production binaries.
pub(crate) struct SettingsPathGuard {
    previous: Option<std::path::PathBuf>,
    _thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl SettingsPathGuard {
    pub(crate) fn take() -> Self {
        Self {
            previous: settings_path(),
            _thread_bound: std::marker::PhantomData,
        }
    }

    pub(crate) fn redirect(&self, home: &std::path::Path) {
        SETTINGS_PATH.with(|slot| *slot.borrow_mut() = Some(home.join(".gwi/settings.json")));
    }
}

impl Drop for SettingsPathGuard {
    fn drop(&mut self) {
        SETTINGS_PATH.with(|slot| *slot.borrow_mut() = self.previous.take());
    }
}

pub(crate) fn settings_path() -> Option<std::path::PathBuf> {
    SETTINGS_PATH.with(|slot| slot.borrow().clone())
}

/// Returns from the calling test when the process runs as root, which
/// bypasses the file-permission checks (DAC) a test relies on: a `0o500`
/// directory is still writable and a file root creates is root-owned. Put it
/// first in any test that makes a path unwritable to force an I/O failure, with
/// a comment saying what the test would otherwise pin. The ordinary `Test` job
/// still runs those tests; `scripts/sandbox-test.sh` runs as root on Linux (a
/// user namespace maps the caller to uid 0), so it skips them. Available only
/// on Unix, matching the permission-denial tests that use it.
///
/// `python3 scripts/check_permission_tests.py` enforces this for literal
/// `from_mode` calls without owner write permission in `src/` and `tests/`.
/// Construct restrictive permissions in the guarded test and pass them to
/// helpers; a helper cannot return early on behalf of its calling test.
/// The required `Doc links` CI job runs the check. Computed modes and control
/// flow still need review: keep this macro first in permission-denial tests.
#[cfg(unix)]
macro_rules! skip_as_root {
    () => {
        if nix::unistd::geteuid().is_root() {
            eprintln!("skipping: needs file-permission checks that root bypasses");
            return;
        }
    };
}

#[cfg(unix)]
pub(crate) use skip_as_root;

/// Redirects the audit log into an isolated tempdir for the life of one
/// test — for this thread only, through `request_log::TEST_AUDIT_ROUTE`,
/// not the process-global `GWI_AUDIT_LOG_FILE` — so it needs no lock
/// and tests holding one run fully in parallel.
///
/// Every Drive lease-check/acquire test that reaches a live lease or a
/// refusal triggers a best-effort or write-ahead audit write (ADR-0080
/// §11) as a side effect of calling production code, whether or not the
/// test cares about its content. A test that doesn't redirect lands in
/// `request_log`'s shared scratch file — never the real machine's
/// `audit.jsonl`, and never another test's env override either, since an
/// un-opted thread does not consult the env var at all; a test that wants
/// to read its *own* records back takes this guard. The writes this guard
/// observes must therefore happen on the test's own thread (a
/// `#[tokio::test]` body does; a `spawn_blocking` closure does not).
///
/// [`Self::records`] reads back what production code wrote, so a test
/// asserting on the audit trail needs no private line-parsing helper.
pub(crate) struct AuditLogGuard {
    path: std::path::PathBuf,
}

impl AuditLogGuard {
    pub(crate) fn redirect(dir: &std::path::Path) -> Self {
        let path = dir.join("audit.jsonl");
        crate::request_log::TEST_AUDIT_ROUTE.with(|slot| *slot.borrow_mut() = Some(path.clone()));
        Self { path }
    }

    /// Every record written so far, in order. An audit file that was never
    /// created (nothing wrote) reads as empty rather than panicking, so a
    /// "must write nothing" assertion is `assert!(guard.records().is_empty())`.
    pub(crate) fn records(&self) -> Vec<crate::request_log::LogRecord> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => text
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => panic!("failed to read {}: {err}", self.path.display()),
        }
    }

    /// The `verdict` of every record written so far, in order — the
    /// assertion almost every audit-trail test makes.
    pub(crate) fn verdicts(&self) -> Vec<String> {
        self.records()
            .iter()
            .map(|record| record.context.get("verdict").cloned().unwrap_or_default())
            .collect()
    }
}

impl Drop for AuditLogGuard {
    fn drop(&mut self) {
        crate::request_log::TEST_AUDIT_ROUTE.with(|slot| *slot.borrow_mut() = None);
    }
}

#[cfg(test)]
mod audit_log_guard_tests {
    use super::AuditLogGuard;

    /// Direct cover for the "nothing wrote" branch of [`AuditLogGuard::records`]
    /// (`ErrorKind::NotFound` reads as empty rather than panicking): every
    /// other caller in the crate redirects and then triggers a write before
    /// reading records back, so this path is otherwise never exercised.
    #[test]
    fn records_reads_as_empty_before_anything_writes() {
        let dir = tempfile::tempdir().unwrap();
        let guard = AuditLogGuard::redirect(dir.path());
        assert!(guard.records().is_empty());
    }

    /// Direct cover for the panic branch of [`AuditLogGuard::records`]: a
    /// read failure other than `NotFound` must not read as "nothing wrote
    /// yet" — it must panic instead, the same fail-loud contract every
    /// other test-support helper in this file follows. A directory in
    /// place of the audit file forces that non-`NotFound` failure, the
    /// same trick `drive::lease::check`'s own fail-closed test uses.
    #[test]
    fn records_panics_when_the_read_fails_for_a_reason_other_than_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let guard = AuditLogGuard::redirect(dir.path());
        std::fs::create_dir(dir.path().join("audit.jsonl")).unwrap();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| guard.records()));
        assert!(result.is_err());
    }
}

/// Installs a request-log environment on this thread only. Like
/// [`AuditLogGuard`], it observes writes on the test thread; spawned tasks
/// need their own route. Dropping restores a nested route, including on panic.
/// Incidental writers on other threads keep their own scratch destination.
pub(crate) struct RequestLogGuard {
    previous: env::MapEnv,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl RequestLogGuard {
    pub(crate) fn redirect(path: &std::path::Path) -> Self {
        Self::with_env(env::MapEnv::new().with("GWI_LOG_FILE", &path.to_string_lossy()))
    }

    pub(crate) fn with_env(env: env::MapEnv) -> Self {
        let previous = crate::request_log::TEST_LOG_ENV
            .with(|slot| std::mem::replace(&mut *slot.borrow_mut(), env));
        Self {
            previous,
            _thread: std::marker::PhantomData,
        }
    }
}

impl Drop for RequestLogGuard {
    fn drop(&mut self) {
        crate::request_log::TEST_LOG_ENV
            .with(|slot| *slot.borrow_mut() = std::mem::take(&mut self.previous));
    }
}

/// Thread-scoped log buffer backing [`capture_at`].
#[derive(Clone, Default)]
struct CaptureWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Keeps one extra dispatcher registered for the life of the process, so that
/// [`capture_at`] and [`capture_future_at`] cannot lose events to a race.
///
/// `tracing-core` caches each callsite's interest. While only *one* dispatcher is
/// registered it recomputes that interest against the *calling thread's* default
/// instead of every registered dispatcher. In the parallel test binary that
/// lets a thread with no subscriber installed — the first to reach a callsite
/// while another thread's capture subscriber is the only one live — cache
/// "never" for it, and the capturing thread's event is then silently dropped:
/// a flake in the capturing test, and a macro line that reports uncovered. A
/// second, never-dropped registration keeps `tracing-core` on the path that
/// consults every live dispatcher. It is never installed as a default, so it
/// changes what no thread logs.
fn keep_every_dispatcher_consulted() {
    static KEEP: std::sync::OnceLock<tracing::Dispatch> = std::sync::OnceLock::new();
    KEEP.get_or_init(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()));
}

/// Runs `f` under a thread-local subscriber that captures every event at
/// `level` or above, and returns everything it logged. `f` must be fully
/// synchronous on this thread. The one shared home for this capture
/// pattern (issue #1744); per-module `capture_info`/`capture_warnings`
/// helpers are thin aliases over it.
pub(crate) fn capture_at(level: tracing::Level, f: impl FnOnce()) -> String {
    keep_every_dispatcher_consulted();
    let writer = CaptureWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_ansi(false)
        .with_writer(writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    let logs = String::from_utf8_lossy(&writer.0.lock().unwrap()).into_owned();
    logs
}

/// Captures events from an async future on every poll, even across worker threads.
/// Spawned child tasks still need their own subscriber; this does not install a
/// global subscriber or hold a thread-local guard across an await.
pub(crate) async fn capture_future_at<F: std::future::Future>(
    level: tracing::Level,
    future: F,
) -> (F::Output, String) {
    use tracing::instrument::WithSubscriber;
    keep_every_dispatcher_consulted();
    let writer = CaptureWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_ansi(false)
        .with_writer(writer.clone())
        .finish();
    let result = future.with_subscriber(subscriber).await;
    let logs = String::from_utf8_lossy(&writer.0.lock().unwrap()).into_owned();
    (result, logs)
}

#[cfg(test)]
mod capture_tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn capture_writer_flush_is_a_no_op() {
        let mut writer = CaptureWriter::default();
        writer.write_all(b"kept").unwrap();
        writer.flush().unwrap();
        assert_eq!(*writer.0.lock().unwrap(), b"kept");
    }

    #[tokio::test]
    async fn capture_future_at_returns_the_output_and_the_logs_at_or_above_level() {
        let (output, logs) = capture_future_at(tracing::Level::INFO, async {
            tracing::debug!("filtered out");
            tracing::info!("captured event");
            7
        })
        .await;
        assert_eq!(output, 7);
        assert!(logs.contains("captured event"), "{logs}");
        assert!(!logs.contains("filtered out"), "{logs}");
    }

    #[test]
    fn while_another_thread_exports_restores_a_value_that_was_already_set() {
        // A key no other test touches, so setting it outside the lock races nothing.
        const KEY: &str = "GWI_TEST_SUPPORT_PRESET_EXPORT";
        std::env::set_var(KEY, "preset");
        while_another_thread_exports(&[(KEY, "exported")], 20, || {
            std::thread::sleep(std::time::Duration::from_micros(200));
        });
        let after = std::env::var(KEY);
        std::env::remove_var(KEY);
        assert_eq!(after.as_deref(), Ok("preset"));
    }
}

pub(crate) mod failing_io {
    //! Writer fixture that always returns `ErrorKind::Other` from
    //! `write` and `flush`. Used to drive `?`-propagation Err branches
    //! in destructive-command tests where the prompt/preview write or
    //! the post-API-success writeln is expected to fail.
    pub(crate) struct FailingWriter;

    impl std::io::Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("simulated write failure"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("simulated flush failure"))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::Write;

        /// Direct cover for `FailingWriter::flush`. The destructive-command
        /// tests fail at the prior `write!` so flush never fires; this
        /// asserts its body still returns the expected error.
        #[test]
        fn write_returns_error() {
            let err = FailingWriter.write(b"x").unwrap_err();
            assert!(err.to_string().contains("simulated write failure"));
        }

        #[test]
        fn flush_returns_error() {
            let mut w = FailingWriter;
            let err = w.flush().unwrap_err();
            assert!(err.to_string().contains("simulated flush failure"));
        }
    }
}

pub(crate) mod env {
    //! Pure in-memory [`EnvSource`](crate::utils::env::EnvSource) for tests.
    //!
    //! `MapEnv` lets env-parsing boundaries be tested without mutating the
    //! process-global environment: a test builds its own map and passes
    //! `&map` to the seam's `*_with(&impl EnvSource, …)` entry point. Because
    //! the map is an owned value with no shared state, such tests need no
    //! lock and run fully in parallel (issue #1030 / #821).
    use std::collections::HashMap;

    /// An [`EnvSource`](crate::utils::env::EnvSource) backed by an in-memory
    /// map — the test counterpart to
    /// [`SystemEnv`](crate::utils::env::SystemEnv).
    #[derive(Debug, Default, Clone)]
    pub(crate) struct MapEnv(HashMap<String, String>);

    impl MapEnv {
        /// Creates an empty environment (every lookup returns `None`).
        pub(crate) fn new() -> Self {
            Self::default()
        }

        /// Inserts `key = value` and returns `self`, for builder-style setup.
        pub(crate) fn with(mut self, key: &str, value: &str) -> Self {
            self.0.insert(key.to_string(), value.to_string());
            self
        }
    }

    impl crate::utils::env::EnvSource for MapEnv {
        fn var(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
    }

    /// Writes `contents` to an owner-only (`0600`) file in a fresh temp dir,
    /// for tests of a secret's `<NAME>_FILE` companion (issue #2006). Returns
    /// the dir (keep it alive) and the file's absolute path.
    #[allow(clippy::unwrap_used)]
    pub(crate) fn secret_file(contents: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret");
        std::fs::write(&path, contents).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let path = path.to_str().unwrap().to_string();
        (dir, path)
    }
}

/// Runs `check` `iterations` times on this thread while a background thread
/// repeatedly exports `vars` — each time under [`HOME_ENV_MUTEX`], then
/// restoring what was there — the regression shape for a test that must see
/// those variables unset: without the lock in `check`, it observes an
/// export and fails intermittently (issue #62, as #17 did for `HOME`).
///
/// `check` takes the `EnvGuard` itself, as the test it stands in for does.
pub(crate) fn while_another_thread_exports(
    vars: &[(&str, &str)],
    iterations: usize,
    check: impl Fn(),
) {
    use std::sync::atomic::{AtomicBool, Ordering};

    let stop = AtomicBool::new(false);

    /// Stops the writer even when `check` panics, so a failing run fails
    /// instead of hanging in the scope's implicit join.
    struct StopOnDrop<'a>(&'a AtomicBool);
    impl Drop for StopOnDrop<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                {
                    let _lock = HOME_ENV_MUTEX
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let before: Vec<_> = vars.iter().map(|(k, _)| std::env::var(k).ok()).collect();
                    for (key, value) in vars {
                        std::env::set_var(key, value);
                    }
                    // Long enough that a checker without the lock is still running.
                    std::thread::sleep(std::time::Duration::from_micros(50));
                    for ((key, _), previous) in vars.iter().zip(before) {
                        match previous {
                            Some(value) => std::env::set_var(key, value),
                            None => std::env::remove_var(key),
                        }
                    }
                }
                // `std::sync::Mutex` is unfair: yield after releasing so the
                // checking thread is not starved of the lock.
                std::thread::yield_now();
            }
        });

        let _stop = StopOnDrop(&stop);
        for _ in 0..iterations {
            check();
        }
    });
}

#[cfg(test)]
mod settings_path_tests {
    use super::*;
    use crate::utils::settings::Settings;

    #[test]
    fn settings_routes_load_independent_fixtures_and_restore_on_drop() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let outer = SettingsPathGuard::take();
        outer.redirect(first.path());
        let path = Settings::get_settings_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"env":{"FIXTURE":"first"}}"#).unwrap();
        assert_eq!(Settings::load().unwrap().env["FIXTURE"], "first");

        {
            let inner = SettingsPathGuard::take();
            inner.redirect(second.path());
            assert!(Settings::load().unwrap().env.is_empty());
            assert_eq!(
                Settings::get_settings_path().unwrap(),
                second.path().join(".gwi/settings.json")
            );
            // A different test thread must never see this thread's route.
            assert!(std::thread::spawn(settings_path).join().unwrap().is_none());
        }
        assert_eq!(Settings::load().unwrap().env["FIXTURE"], "first");
        drop(outer);
        assert!(settings_path().is_none());
    }
}

/// Textual enforcement for the environment keys shared by Gmail and Drive tests.
/// Dynamic key expressions and indirect helper calls are outside this heuristic.
#[cfg(test)]
mod env_guard_tests {
    use std::path::{Path, PathBuf};

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

    fn unguarded_mutations(path: &Path, text: &str) -> Vec<String> {
        let mutation = regex::Regex::new(
            r#"(?:set_var|remove_var)\s*\(\s*"?(?:[A-Za-z_][A-Za-z0-9_]*::)*(?:GMAIL_[A-Z_]+|DRIVE_[A-Z_]+|GWI_(?:PROFILE|GMAIL_ACCOUNT|DRIVE_ACCOUNT|DRIVE_LEASE_[A-Z_]+)|LEASE_[A-Z_]+_ENV|HOME|PROFILE_ENV_VAR|SHEETS_API_URL|DOCS_API_URL|SLIDES_API_URL)\b"#,
        ).unwrap();
        // Splitting on `fn ` is a heuristic: a nested function can hide a
        // preceding guard, producing a loud failure rather than a missed race.
        text.split("fn ")
            .skip(1)
            .filter_map(|body| {
                let name = body.split(['(', '<']).next().unwrap_or_default().trim();
                let guard_helper = (path == Path::new("gmail/test_support.rs")
                    || path == Path::new("drive/test_support.rs"))
                    && name == "clear_credentials"
                    || path == Path::new("drive/test_support.rs")
                        && name == "redirect_api_hosts_to_a_dead_port";
                let profile_propagation = path == Path::new("cli.rs")
                    && name == "propagate_profile_flag"
                    && mutation.find_iter(body).all(|found| {
                        found.as_str().ends_with("PROFILE_ENV_VAR")
                            || found.as_str().ends_with("GWI_PROFILE")
                    });
                if mutation.is_match(body)
                    && !body.contains("EnvGuard::take()")
                    && !guard_helper
                    && !profile_propagation
                {
                    Some(format!("{}: fn {name}", path.display()))
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn every_gmail_and_drive_env_mutation_holds_the_env_guard() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_sources(&src, &mut files);
        let mut offenders = Vec::new();
        // This file contains source fixtures and the lock-holding chaos helper
        // which mutates dynamic keys; neither is a domain test to scan.
        for path in files
            .into_iter()
            .filter(|p| *p != src.join("test_support.rs"))
        {
            let text = std::fs::read_to_string(&path).unwrap();
            offenders.extend(unguarded_mutations(path.strip_prefix(&src).unwrap(), &text));
        }
        assert!(
            offenders.is_empty(),
            "these functions mutate a shared env var without `EnvGuard::take()`:\n{}",
            offenders.join("\n") // patchcov: coverage ignore-line reason="assert! message args only evaluate when an unguarded mutation is found"
        );
    }

    #[test]
    fn the_scan_rejects_unguarded_mutations_and_accepts_guarded_ones() {
        for key in [
            "GMAIL_CLIENT_ID",
            "GMAIL_CLIENT_SECRET_FILE",
            "GMAIL_REFRESH_TOKEN_COMMAND",
            "DRIVE_CLIENT_SECRET_FILE",
            "GMAIL_ACCOUNT_ENV",
            "DRIVE_ACCOUNT_ENV",
            "SHEETS_API_URL",
            "DOCS_API_URL",
            "SLIDES_API_URL",
            "PROFILE_ENV_VAR",
            "crate::utils::settings::PROFILE_ENV_VAR",
            "\"HOME\"",
            "\"GWI_PROFILE\"",
            "\"GWI_GMAIL_ACCOUNT\"",
            "\"GWI_DRIVE_ACCOUNT\"",
            "\"GMAIL_SCOPE\"",
            "LEASE_EXPIRY_MINUTES_ENV",
            "crate::drive::lease::settings::LEASE_BACKUP_DIR_ENV",
            "\"GWI_DRIVE_LEASE_BIOMETRICS_ONLY\"",
            "\"GWI_DRIVE_LEASE_ALLOW_HEADLESS\"",
        ] {
            for call in [
                format!("std::env::set_var({key}, \"value\");"),
                format!("std::env::remove_var({key});"),
            ] {
                let source = format!("fn example() {{ {call} }}");
                assert_eq!(
                    unguarded_mutations(Path::new("gmail/example.rs"), &source),
                    ["gmail/example.rs: fn example"],
                    "{call}"
                );
                let guarded = format!("fn example() {{ let _guard = EnvGuard::take(); {call} }}");
                assert!(
                    unguarded_mutations(Path::new("gmail/example.rs"), &guarded).is_empty(),
                    "{call}"
                );
            }
        }
    }

    #[test]
    fn exemptions_apply_only_to_the_named_helper_in_its_own_file() {
        let extra_mutation =
            "fn propagate_profile_flag() { std::env::set_var(GMAIL_SCOPE, \"value\"); }";
        assert_eq!(
            unguarded_mutations(Path::new("cli.rs"), extra_mutation).len(),
            1
        );
        // Repository paths use native separators, including backslashes on Windows.
        for domain in ["gmail", "drive"] {
            let native_path = Path::new(domain).join("test_support.rs");
            let helper = "fn clear_credentials() { std::env::set_var(\"HOME\", \"value\"); }";
            assert!(unguarded_mutations(&native_path, helper).is_empty());
        }
        for (path, name, key) in [
            ("cli.rs", "propagate_profile_flag", "PROFILE_ENV_VAR"),
            ("gmail/test_support.rs", "clear_credentials", "\"HOME\""),
            ("drive/test_support.rs", "clear_credentials", "\"HOME\""),
            (
                "drive/test_support.rs",
                "redirect_api_hosts_to_a_dead_port",
                "DRIVE_API_URL",
            ),
        ] {
            let source = format!("fn {name}() {{ std::env::set_var({key}, \"value\"); }}");
            assert!(unguarded_mutations(Path::new(path), &source).is_empty());
            assert_eq!(unguarded_mutations(Path::new("other.rs"), &source).len(), 1);
            let other_function = source.replace(name, "unguarded_test");
            assert_eq!(
                unguarded_mutations(Path::new(path), &other_function).len(),
                1
            );
        }
    }
}
