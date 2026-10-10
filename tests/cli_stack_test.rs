//! Isolated CLI stack probe: a stack overflow must only kill the child process.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use clap::{CommandFactory, Parser};
use gwi::Cli;

/// Run directly with `GWI_TEST_STACK_KIB=<budget>` and `--ignored --exact stack_probe`.
#[test]
#[ignore = "measurement child; invoked in a separate process by stack_budget"]
fn stack_probe() {
    let kib: usize = std::env::var("GWI_TEST_STACK_KIB")
        .unwrap()
        .parse()
        .unwrap();
    std::thread::Builder::new()
        .stack_size(kib * 1024)
        .spawn(|| {
            Cli::command().debug_assert();
            Cli::command_for_update().debug_assert();
            for args in [vec!["gwi", "--help"], vec!["gwi", "--version"]] {
                let err = Cli::try_parse_from(args).err().unwrap();
                assert!(matches!(
                    err.kind(),
                    clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
                ));
            }
            let log = Cli::try_parse_from(["gwi", "log", "--limit", "0"]).unwrap();
            assert!(matches!(log.command, gwi::cli::Commands::Log(_)));
            let mut deep = Cli::try_parse_from([
                "gwi",
                "drive",
                "sheets",
                "read",
                "spreadsheet",
                "--range",
                "A1:C10",
            ])
            .unwrap();
            deep.try_update_from([
                "gwi",
                "drive",
                "sheets",
                "read",
                "spreadsheet",
                "--profile",
                "work",
            ])
            .unwrap();
            assert_eq!(deep.profile.as_deref(), Some("work"));
            assert!(matches!(deep.command, gwi::cli::Commands::Drive(_)));
            assert_eq!(
                Cli::try_parse_from(["gwi", "log", "--unknown"])
                    .err()
                    .unwrap()
                    .kind(),
                clap::error::ErrorKind::UnknownArgument
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn stack_budget() {
    // Windows still needs a measured budget; keep the known-safe reserve until
    // the before/after Windows sweep proves a smaller budget for the full tree.
    let kib = if cfg!(windows) { 8192 } else { 1024 };
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .env("GWI_TEST_STACK_KIB", kib.to_string())
        .args(["--ignored", "--exact", "stack_probe", "--nocapture"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "CLI stack probe failed at {kib} KiB: {}\n{stdout}\n{stderr}",
        output.status
    );
}
