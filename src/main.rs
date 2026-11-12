//! `gwi`, the Google Workspace Interface.
//!
//! Entry point ported from omni-dev's `main.rs` (rust-works/omni-dev#2203),
//! without the daemon and menu-bar handling: it initialises tracing, installs
//! the per-invocation request-log context, runs the CLI on a tokio runtime and
//! appends one invocation record before exiting.

use std::process;
use std::time::Instant;

use clap::{CommandFactory, Parser};
use gwi::request_log::{self, InvocationOutcome, RequestLogContext, Source};
use gwi::Cli;

fn main() {
    // Before anything else, so the timestamp is as close to process entry as Rust allows.
    record_startup_milestone();

    // Capture argv before clap consumes it, so the invocation record can log the
    // full command line and the resolved subcommand path.
    let argv: Vec<String> = std::env::args().collect();
    let command = resolve_command_path(&argv);

    init_tracing();

    let cli = Cli::parse();

    // Install the per-invocation context up front; `set_global` is
    // first-write-wins, and HTTP records inherit it.
    request_log::set_global(RequestLogContext {
        invocation_id: request_log::new_id(),
        source: Source::Cli,
        mcp_tool: None,
    });

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("Error: failed to start the tokio runtime: {e}");
            process::exit(1);
        }
    };

    // Time the whole command and append one invocation record after it returns.
    // Logging is best-effort and never affects the exit code.
    let start = Instant::now();
    let result = runtime.block_on(cli.execute());

    let (exit_code, error) = match &result {
        Ok(()) => (0, None),
        Err(e) => (1, Some(format!("{e:#}"))),
    };
    request_log::record_invocation(InvocationOutcome {
        command,
        command_line: argv,
        exit_code,
        error,
        duration: start.elapsed(),
    });

    if let Err(e) = result {
        die(&e);
    }
}

/// Environment variable naming a file that receives a timestamp at entry to `main`.
///
/// Opt-in diagnostic for the log-follow probe (#309): the probe compares it with
/// its own spawn time to separate exec/loader delay from everything after `main`.
/// Unset, which is every ordinary run, nothing is written.
const STARTUP_MILESTONE_ENV: &str = "GWI_STARTUP_MILESTONE_FILE";

/// Writes the `main`-entry milestone to the file named by [`STARTUP_MILESTONE_ENV`].
fn record_startup_milestone() {
    if let Some(path) = std::env::var_os(STARTUP_MILESTONE_ENV) {
        write_startup_milestone(std::path::Path::new(&path));
    }
}

/// Atomically writes `{"pid":…,"main_unix_seconds":…}` to `path`.
///
/// Best effort: a diagnostic must never change stdout, stderr or the exit code,
/// so every failure is ignored. The write is a temporary file renamed into place,
/// so a reader never sees a partial milestone.
fn write_startup_milestone(path: &std::path::Path) {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64());
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}.tmp", process::id()));
    let temporary = std::path::PathBuf::from(temporary);
    let body = format!(
        "{{\"pid\":{},\"main_unix_seconds\":{seconds:.6}}}\n",
        process::id()
    );
    if std::fs::write(&temporary, body)
        .and_then(|()| std::fs::rename(&temporary, path))
        .is_err()
    {
        let _ = std::fs::remove_file(&temporary);
    }
}

/// Resolves the clap subcommand path (e.g. `["gmail","search"]`) by re-deriving
/// matches from argv and walking the subcommand chain. Generic, so it stays
/// correct as subcommands are added, and returns an empty path if re-parsing
/// fails.
fn resolve_command_path(argv: &[String]) -> Vec<String> {
    let mut path = Vec::new();
    let Ok(matches) = Cli::command().try_get_matches_from(argv) else {
        return path;
    };
    let mut current = &matches;
    while let Some((name, sub)) = current.subcommand() {
        path.push(name.to_string());
        current = sub;
    }
    path
}

/// Initialises the tracing subscriber (stderr, `RUST_LOG`-driven), keeping logs
/// off stdout. The default level when `RUST_LOG` is unset, or invalid, is `warn`.
fn init_tracing() {
    let filter = std::env::var("RUST_LOG")
        .ok()
        .and_then(|directive| tracing_subscriber::EnvFilter::try_new(directive).ok())
        .unwrap_or_else(|| tracing_subscriber::EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .init();
}

/// Prints an error and its source chain to stderr, then exits non-zero.
fn die(e: &anyhow::Error) -> ! {
    eprintln!("Error: {e}");
    let mut source = e.source();
    while let Some(err) = source {
        eprintln!("  Caused by: {err}");
        source = err.source();
    }
    process::exit(1);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Tests assert on fixtures they create themselves.
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn the_command_path_follows_the_subcommand_chain() {
        assert_eq!(
            resolve_command_path(&argv(&["gwi", "gmail", "auth", "status"])),
            ["gmail", "auth", "status"]
        );
        // Flags and values around the chain do not change it.
        assert_eq!(
            resolve_command_path(&argv(&[
                "gwi",
                "--profile",
                "work",
                "gmail",
                "label",
                "list"
            ])),
            ["gmail", "label", "list"]
        );
    }

    #[test]
    fn the_startup_milestone_records_pid_and_time() {
        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.json");

        write_startup_milestone(&path);

        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["pid"], process::id());
        assert!(value["main_unix_seconds"].as_f64().unwrap() >= before - 0.001);
        // The temporary file is renamed away, leaving only the milestone.
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn an_unwritable_startup_milestone_is_ignored() {
        let directory = tempfile::tempdir().unwrap();
        // The parent directory does not exist: nothing is written and nothing panics.
        write_startup_milestone(&directory.path().join("missing").join("main.json"));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn an_unparseable_command_line_resolves_to_an_empty_path() {
        assert!(resolve_command_path(&argv(&["gwi"])).is_empty());
        assert!(resolve_command_path(&argv(&["gwi", "--no-such-flag"])).is_empty());
    }
}
