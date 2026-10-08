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
#[expect(
    dead_code,
    reason = "used by the Drive tests, wired in with the Drive slice"
)]
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
