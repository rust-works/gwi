//! The one place a browser (or any browser-opener command) is started.
//!
//! `gmail::auth` and `drive::auth` open the OAuth authorization URL through
//! [`launch_detached`] and never spawn a process themselves, so a test can replace
//! the launch and a test that reaches one it did not expect fails.
//!
//! Under `cfg(test)` the call is routed to a launcher the test installs with
//! [`testing::LaunchGuard::install`], and **panics when none is installed**. That
//! fails the test on every platform, whatever the opener would have done: the sandbox
//! in `scripts/sandbox-test.sh` can only block an opener it knows the name of, and the
//! real launch discards the opener's exit status, so neither could be relied on.
//!
//! The replacement is per thread (as `request_log::TEST_AUDIT_ROUTE` is), so tests
//! run in parallel. It follows a `#[tokio::test]` body, which runs on the test's own
//! thread; a launch from another thread finds no launcher and panics.

use std::process::{Command, Stdio};

use anyhow::{Context, Result};

/// Starts `program` with `args`, detached from this process's stdio, without waiting
/// for it or looking at its exit status.
///
/// # Errors
///
/// If the process cannot be started.
#[cfg(not(test))]
pub(crate) fn launch_detached(program: &str, args: &[String]) -> Result<()> {
    spawn_detached(program, args)
}

/// The `cfg(test)` build of [`launch_detached`]: records the launch, or panics.
///
/// # Errors
///
/// Never, but it shares the real function's signature.
#[cfg(test)]
pub(crate) fn launch_detached(program: &str, args: &[String]) -> Result<()> {
    testing::dispatch(program, args)
}

/// The real launch behind [`launch_detached`].
fn spawn_detached(program: &str, args: &[String]) -> Result<()> {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .context("Failed to launch the browser")
}

/// Test-only replacement for the launch.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
pub(crate) mod testing {
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use anyhow::Result;

    /// One launch a test intercepted.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) struct Launch {
        pub(crate) program: String,
        pub(crate) args: Vec<String>,
    }

    /// How long [`RecordedLaunches::wait_for_first`] waits for a launch.
    const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

    thread_local! {
        static LAUNCHER: RefCell<Option<RecordedLaunches>> = const { RefCell::new(None) };
    }

    /// The launches a [`LaunchGuard`] has recorded. Cloning shares the record, so a
    /// task spawned by the test can wait on it.
    #[derive(Clone, Debug, Default)]
    pub(crate) struct RecordedLaunches(Arc<Mutex<Vec<Launch>>>);

    impl RecordedLaunches {
        pub(crate) fn calls(&self) -> Vec<Launch> {
            self.0.lock().unwrap().clone()
        }

        /// Waits for the first launch and returns it.
        ///
        /// # Panics
        ///
        /// If none is recorded within [`WAIT_TIMEOUT`], so a test whose code never
        /// launches fails instead of hanging.
        pub(crate) async fn wait_for_first(&self) -> Launch {
            let wait = async {
                loop {
                    if let Some(first) = self.0.lock().unwrap().first() {
                        return first.clone();
                    }
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            };
            tokio::time::timeout(WAIT_TIMEOUT, wait)
                .await
                .expect("no browser launch was recorded")
        }
    }

    /// Replaces the launch on this thread with a recorder until dropped, then restores
    /// what was installed before (the default, which panics, unless guards are nested).
    ///
    /// Drop it on the thread that installed it (a test body does; `future_not_send` would
    /// reject a `!Send` guard held across an `.await`).
    #[must_use = "the recorder is removed when the guard is dropped"]
    pub(crate) struct LaunchGuard {
        recorded: RecordedLaunches,
        previous: Option<RecordedLaunches>,
    }

    impl LaunchGuard {
        pub(crate) fn install() -> Self {
            let recorded = RecordedLaunches::default();
            let previous = LAUNCHER.with(|slot| slot.borrow_mut().replace(recorded.clone()));
            Self { recorded, previous }
        }

        pub(crate) fn launches(&self) -> RecordedLaunches {
            self.recorded.clone()
        }
    }

    impl Drop for LaunchGuard {
        fn drop(&mut self) {
            let previous = self.previous.take();
            LAUNCHER.with(|slot| *slot.borrow_mut() = previous);
        }
    }

    pub(super) fn dispatch(program: &str, args: &[String]) -> Result<()> {
        let Some(recorded) = LAUNCHER.with(|slot| slot.borrow().clone()) else {
            panic!(
                "unexpected browser launch: {program} {args:?}; a test that expects one \
                 installs a recorder with LaunchGuard::install()"
            );
        };
        recorded.0.lock().unwrap().push(Launch {
            program: program.to_string(),
            args: args.to_vec(),
        });
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::testing::{Launch, LaunchGuard};
    use super::*;

    fn args(args: &[&str]) -> Vec<String> {
        args.iter().map(ToString::to_string).collect()
    }

    #[test]
    #[should_panic(expected = "unexpected browser launch: xdg-open")]
    fn a_launch_with_no_recorder_installed_panics() {
        let _ = launch_detached("xdg-open", &args(&["https://example/auth"]));
    }

    #[test]
    fn an_installed_recorder_captures_the_launch_instead_of_running_it() {
        let guard = LaunchGuard::install();

        // Not a program that exists: recording must not try to start it.
        launch_detached(
            "no-such-browser",
            &args(&["--new-window", "https://example"]),
        )
        .unwrap();

        assert_eq!(
            guard.launches().calls(),
            vec![Launch {
                program: "no-such-browser".to_string(),
                args: args(&["--new-window", "https://example"]),
            }]
        );
    }

    #[test]
    #[should_panic(expected = "unexpected browser launch")]
    fn dropping_the_guard_restores_the_panicking_default() {
        drop(LaunchGuard::install());
        let _ = launch_detached("open", &args(&["https://example"]));
    }

    #[test]
    fn a_nested_guard_restores_the_outer_recorder_when_dropped() {
        let outer = LaunchGuard::install();
        drop(LaunchGuard::install());

        launch_detached("browser", &args(&["https://example"])).unwrap();

        assert_eq!(outer.launches().calls().len(), 1);
    }

    #[tokio::test]
    async fn wait_for_first_sees_a_launch_made_after_it_started_waiting() {
        let guard = LaunchGuard::install();
        let launches = guard.launches();
        let waiter = tokio::spawn(async move { launches.wait_for_first().await });

        launch_detached("browser", &args(&["https://example"])).unwrap();

        assert_eq!(waiter.await.unwrap().program, "browser");
    }

    #[cfg(unix)]
    #[test]
    fn the_real_launch_starts_a_process() {
        // `true` ignores its arguments and opens nothing.
        spawn_detached("true", &args(&["https://example"])).unwrap();
    }

    #[test]
    fn the_real_launch_reports_a_program_that_cannot_be_started() {
        let err = spawn_detached("gwi-no-such-browser-program", &[]).unwrap_err();
        assert!(err.to_string().contains("Failed to launch the browser"));
    }
}
