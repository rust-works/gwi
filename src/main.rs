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
    fn an_unparseable_command_line_resolves_to_an_empty_path() {
        assert!(resolve_command_path(&argv(&["gwi"])).is_empty());
        assert!(resolve_command_path(&argv(&["gwi", "--no-such-flag"])).is_empty());
    }
}
