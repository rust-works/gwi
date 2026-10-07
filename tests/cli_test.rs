//! Command-line surface tests: a golden snapshot of every command's help, and
//! smoke tests that run the real binary.
//!
//! The binary tests never touch the network or a browser: they run with an empty
//! `HOME`, no ambient Google credentials and the request log redirected into that
//! directory, and only exercise `--help`, `--version`, usage errors and the
//! "not configured" failure that precedes any API call.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::process::{Command, Output};

use clap::CommandFactory;
use gwi::Cli;

/// Renders the long help of `cmd` and of every subcommand beneath it, depth first.
fn render_all(cmd: &mut clap::Command, out: &mut String) {
    let heading = cmd
        .get_bin_name()
        .unwrap_or_else(|| cmd.get_name())
        .to_string();
    out.push_str(&format!("===== {heading} =====\n"));
    out.push_str(&cmd.render_long_help().to_string());
    out.push('\n');
    for sub in cmd.get_subcommands_mut() {
        if sub.get_name() != "help" {
            render_all(sub, out);
        }
    }
}

#[test]
fn help_all_golden() {
    let mut root = Cli::command();
    root.build();
    let mut help = String::new();
    render_all(&mut root, &mut help);

    insta::assert_snapshot!("help_all_output", help);
}

/// Runs the real `gwi` binary hermetically in `home`.
fn gwi(home: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gwi"));
    command
        .args(args)
        .env("HOME", home)
        .env("GWI_LOG_FILE", home.join("log.jsonl"))
        .env("GWI_LOG_DISABLE", "1");
    for ambient in [
        "GMAIL_CLIENT_ID",
        "GMAIL_CLIENT_SECRET",
        "GMAIL_CLIENT_SECRET_FILE",
        "GMAIL_REFRESH_TOKEN",
        "GMAIL_REFRESH_TOKEN_FILE",
        "GMAIL_REFRESH_TOKEN_COMMAND",
        "GWI_PROFILE",
        "GWI_GMAIL_ACCOUNT",
        "GWI_CONFIG_DIR",
    ] {
        command.env_remove(ambient);
    }
    command.output().expect("failed to run the gwi binary")
}

#[test]
fn binary_help_and_version_succeed() {
    let home = tempfile::tempdir().unwrap();

    let help = gwi(home.path(), &["--help"]);
    assert!(help.status.success());
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.contains("Google Workspace Interface"), "{stdout}");
    assert!(stdout.contains("gmail"), "{stdout}");

    let version = gwi(home.path(), &["--version"]);
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("gwi "));
}

#[test]
fn binary_rejects_a_missing_or_unknown_command() {
    let home = tempfile::tempdir().unwrap();

    for args in [&[][..], &["no-such-command"], &["gmail", "no-such-command"]] {
        let output = gwi(home.path(), args);
        assert!(!output.status.success(), "{args:?} should fail");
        assert_eq!(output.status.code(), Some(2), "{args:?} is a usage error");
    }
}

#[test]
fn binary_reports_unconfigured_credentials_before_any_api_call() {
    let home = tempfile::tempdir().unwrap();

    let output = gwi(home.path(), &["gmail", "auth", "status"]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not configured"), "{stderr}");
}

#[test]
fn binary_rejects_an_unknown_profile_before_dispatch() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".gwi")).unwrap();
    std::fs::write(
        home.path().join(".gwi").join("settings.json"),
        r#"{"profiles":{"work":{"env":{}}}}"#,
    )
    .unwrap();

    let output = gwi(
        home.path(),
        &["--profile", "wrok", "gmail", "auth", "status"],
    );

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("wrok") && stderr.contains("work"),
        "{stderr}"
    );
}

/// A fixture settings file shaped like omni-dev's, holding one Gmail account.
fn write_omni_dev_settings(home: &Path) {
    let dir = home.join(".omni-dev");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("settings.json"),
        r#"{"env":{"ATLASSIAN_API_TOKEN":"not-google"},
            "gmail":{"default_account":"work","accounts":{"work":{
                "client_id":"fixture-id","refresh_token":"fixture-token-value",
                "scope":"gmail.readonly"}}}}"#,
    )
    .unwrap();
}

#[test]
fn import_brings_an_omni_dev_account_across_without_touching_the_source() {
    let home = tempfile::tempdir().unwrap();
    write_omni_dev_settings(home.path());
    let source = home.path().join(".omni-dev").join("settings.json");
    let source_before = std::fs::read(&source).unwrap();
    let list = |home: &Path| gwi(home, &["gmail", "account", "list"]);

    // gwi cannot see omni-dev's accounts on its own.
    assert!(!String::from_utf8_lossy(&list(home.path()).stdout).contains("work"));

    // A dry run reports the plan and writes nothing, not even the directory.
    let dry = gwi(home.path(), &["import", "--dry-run"]);
    assert!(dry.status.success());
    assert!(String::from_utf8_lossy(&dry.stdout).contains("Dry run, nothing written"));
    assert!(!home.path().join(".gwi").exists());

    let imported = gwi(home.path(), &["import"]);
    assert!(imported.status.success());
    let report = String::from_utf8_lossy(&imported.stdout);
    assert!(
        report.contains("added       gmail.accounts.work"),
        "{report}"
    );
    assert!(!report.contains("fixture-token-value"), "{report}");
    assert!(
        !String::from_utf8_lossy(&imported.stderr).contains("fixture-token-value"),
        "secrets must not appear on stderr either"
    );

    // Now gwi lists the account, as the default.
    let listed = String::from_utf8_lossy(&list(home.path()).stdout).into_owned();
    assert!(
        listed.contains("work") && listed.contains("gmail.readonly"),
        "{listed}"
    );

    // A second import is a no-op, and omni-dev's file never changed.
    let again = gwi(home.path(), &["import"]);
    assert!(again.status.success());
    assert!(String::from_utf8_lossy(&again.stdout).contains("0 added, 0 overwritten, 2 unchanged"));
    assert_eq!(std::fs::read(&source).unwrap(), source_before);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(home.path().join(".gwi").join("settings.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn import_fails_on_a_conflict_until_forced() {
    let home = tempfile::tempdir().unwrap();
    write_omni_dev_settings(home.path());
    std::fs::create_dir_all(home.path().join(".gwi")).unwrap();
    let target = home.path().join(".gwi").join("settings.json");
    std::fs::write(
        &target,
        r#"{"gmail":{"accounts":{"work":{"client_id":"already-in-gwi"}}}}"#,
    )
    .unwrap();

    let refused = gwi(home.path(), &["import"]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--force"));
    assert!(std::fs::read_to_string(&target)
        .unwrap()
        .contains("already-in-gwi"));

    let forced = gwi(home.path(), &["import", "--force"]);
    assert!(forced.status.success());
    assert!(std::fs::read_to_string(&target)
        .unwrap()
        .contains("fixture-id"));
}

#[test]
fn import_without_a_source_file_says_how_to_find_one() {
    let home = tempfile::tempdir().unwrap();

    let output = gwi(home.path(), &["import"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--source"));
}
