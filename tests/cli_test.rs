//! Command-line surface tests: a golden snapshot of every command's help, and
//! smoke tests that run the real binary.
//!
//! The binary tests never touch the network or a browser: they run with an empty
//! `HOME`, no ambient Google credentials and the request log redirected into that
//! directory, and only exercise `--help`, `--version`, usage errors and the
//! "not configured" failure that precedes any API call.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

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

/// Help is read in a terminal, where a markdown link such as
/// `[ADR-0066](../../docs/adrs/adr-0066.md)` is unreadable and cannot resolve:
/// doc comments that clap turns into help name ADRs in plain text.
#[test]
fn help_has_no_markdown_links() {
    let mut root = Cli::command();
    root.build();
    let mut help = String::new();
    render_all(&mut root, &mut help);

    let links: Vec<&str> = help.lines().filter(|line| line.contains("](")).collect();
    assert!(links.is_empty(), "markdown links in help: {links:#?}");
}

/// URL identifier hints are plain text in both terminal help formats.
#[test]
fn identifier_hints_have_no_markdown() {
    fn check(cmd: &mut clap::Command) -> usize {
        let mut hints = 0;
        for help in [cmd.render_help(), cmd.render_long_help()] {
            for line in help
                .to_string()
                .lines()
                .filter(|line| line.contains("/d/<ID>/"))
            {
                assert!(
                    !line.contains('`'),
                    "markdown in {} help: {line}",
                    cmd.get_name()
                );
                hints += 1;
            }
        }
        for sub in cmd.get_subcommands_mut() {
            hints += check(sub);
        }
        hints
    }

    let mut root = Cli::command();
    root.build();
    assert_eq!(
        check(&mut root),
        182,
        "91 identifier hints in both help formats"
    );
}

/// Imported issue references must name their repository in terminal help.
#[test]
fn help_has_no_bare_four_digit_issue_references() {
    fn check(cmd: &mut clap::Command, bare_issue: &regex::Regex) {
        for help in [cmd.render_help(), cmd.render_long_help()] {
            let help = help.to_string();
            let lines: Vec<&str> = help
                .lines()
                .filter(|line| bare_issue.is_match(line))
                .collect();
            assert!(
                lines.is_empty(),
                "bare issue references in {} help: {lines:#?}",
                cmd.get_name()
            );
        }
        for sub in cmd.get_subcommands_mut() {
            check(sub, bare_issue);
        }
    }

    let bare_issue = regex::Regex::new(r"(?:^|[^\w/-])#[0-9]{4}\b").unwrap();
    let mut root = Cli::command();
    root.build();
    check(&mut root, &bare_issue);
}

/// Runs the real `gwi` binary hermetically in `home`.
fn gwi(home: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gwi"));
    common::scrub_ambient_env(&mut command)
        .args(args)
        .env("HOME", home);
    common::pin_log_env(&mut command, home);
    command.output().expect("failed to run the gwi binary")
}

#[test]
fn pin_log_env_replaces_the_variables_a_developer_shell_exports() {
    let home = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_gwi"));
    command
        .env("GWI_LOG_FILE", "exported-log.jsonl")
        .env("GWI_AUDIT_LOG_FILE", "exported-audit.jsonl")
        .env("GWI_LOG_DISABLE", "0");
    common::pin_log_env(&mut command, home.path());
    for (name, expected) in [
        (
            "GWI_LOG_FILE",
            home.path().join("log.jsonl").into_os_string(),
        ),
        (
            "GWI_AUDIT_LOG_FILE",
            home.path().join("audit.jsonl").into_os_string(),
        ),
        ("GWI_LOG_DISABLE", std::ffi::OsString::from("1")),
    ] {
        let entry = command.get_envs().find(|(key, _)| *key == name);
        assert_eq!(
            entry,
            Some((std::ffi::OsStr::new(name), Some(expected.as_os_str()))),
            "{name}"
        );
    }
}

#[test]
fn scrub_ambient_env_removes_the_variables_a_developer_shell_exports() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gwi"));
    command
        .env("GWI_LOG_MAX_SIZE", "1")
        .env("GWI_HTTP_READ_TIMEOUT_SECS", "1")
        .env("GWI_SECRET_COMMAND_TTL_SECS", "0")
        .env("GWI_DRIVE_LEASE_BIOMETRICS_ONLY", "1")
        .env("GWI_PROFILE", "exported")
        .env("DRIVE_API_URL", "http://127.0.0.1:1")
        .env("GMAIL_REFRESH_TOKEN_COMMAND", "echo exported");
    common::scrub_ambient_env(&mut command);
    for name in [
        "GWI_LOG_MAX_SIZE",
        "GWI_HTTP_READ_TIMEOUT_SECS",
        "GWI_SECRET_COMMAND_TTL_SECS",
        "GWI_DRIVE_LEASE_BIOMETRICS_ONLY",
        "GWI_PROFILE",
        "DRIVE_API_URL",
        "GMAIL_REFRESH_TOKEN_COMMAND",
    ] {
        let entry = command.get_envs().find(|(key, _)| *key == name);
        assert_eq!(entry, Some((std::ffi::OsStr::new(name), None)), "{name}");
    }
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
fn import_dry_run_exits_zero_on_a_settings_conflict() {
    let home = tempfile::tempdir().unwrap();
    write_omni_dev_settings(home.path());
    std::fs::create_dir_all(home.path().join(".gwi")).unwrap();
    let target = home.path().join(".gwi").join("settings.json");
    let existing = r#"{"gmail":{"accounts":{"work":{"client_id":"already-in-gwi"}}}}"#;
    std::fs::write(&target, existing).unwrap();

    let preview = gwi(home.path(), &["import", "--dry-run"]);
    assert!(preview.status.success(), "{preview:?}");
    let stdout = String::from_utf8_lossy(&preview.stdout);
    assert!(
        stdout.contains("conflict    gmail.accounts.work"),
        "{stdout}"
    );
    assert!(stdout.contains("1 conflict(s)"), "{stdout}");
    assert_eq!(
        stdout.matches("this dry run succeeds").count(),
        1,
        "{stdout}"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), existing);

    assert_eq!(gwi(home.path(), &["import"]).status.code(), Some(1));
}

#[test]
fn import_dry_run_exits_zero_on_a_ledger_conflict() {
    let home = tempfile::tempdir().unwrap();
    write_omni_dev_settings(home.path());
    let elsewhere = home.path().join("elsewhere.jsonl");
    let row = r#"{"token":"t1","file_id":"f","version":"1","backup":{"kind":"drive_copy","file_id":"c"},"acquired_at":"2020-01-01T00:00:00Z","expires_at":"2020-01-01T00:15:00Z"}"#;
    std::fs::write(&elsewhere, row).unwrap();
    let source_ledger = elsewhere.to_str().unwrap();
    assert!(
        gwi(home.path(), &["import", "--source-ledger", source_ledger])
            .status
            .success()
    );
    std::fs::write(&elsewhere, row.replace("\"1\"", "\"2\"")).unwrap();
    let target = state_dir(home.path())
        .join("gwi")
        .join("lease-ledger.jsonl");
    let before = std::fs::read(&target).unwrap();

    let preview = gwi(
        home.path(),
        &["import", "--dry-run", "--source-ledger", source_ledger],
    );
    assert!(preview.status.success(), "{preview:?}");
    let stdout = String::from_utf8_lossy(&preview.stdout);
    assert!(stdout.contains("conflict    lease t1"), "{stdout}");
    assert_eq!(
        stdout.matches("this dry run succeeds").count(),
        1,
        "{stdout}"
    );
    assert_eq!(std::fs::read(&target).unwrap(), before);

    let real = gwi(home.path(), &["import", "--source-ledger", source_ledger]);
    assert_eq!(real.status.code(), Some(1));
}

/// Where `dirs` puts the state directory when `HOME` is `home`.
fn state_dir(home: &Path) -> std::path::PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library").join("Application Support")
    } else {
        home.join(".local").join("state")
    }
}

/// A fixture omni-dev lease ledger holding one live and one expired lease.
fn write_omni_dev_ledger(home: &Path) -> std::path::PathBuf {
    let path = state_dir(home).join("omni-dev").join("lease-ledger.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let row = |token: &str, expires_at: &str| {
        format!(
            r#"{{"token":"{token}","file_id":"file-1","version":"4","backup":{{"kind":"drive_copy","file_id":"copy-1"}},"acquired_at":"2020-01-01T00:00:00Z","expires_at":"{expires_at}"}}"#
        )
    };
    std::fs::write(
        &path,
        format!(
            "{}\n{}\n",
            row("lease-expired", "2020-01-01T00:15:00Z"),
            row("lease-live", "2999-01-01T00:00:00Z")
        ),
    )
    .unwrap();
    path
}

#[test]
fn import_carries_the_lease_ledger_across_and_a_second_run_changes_nothing() {
    let home = tempfile::tempdir().unwrap();
    write_omni_dev_settings(home.path());
    let source = write_omni_dev_ledger(home.path());
    let source_before = std::fs::read(&source).unwrap();
    let target = state_dir(home.path())
        .join("gwi")
        .join("lease-ledger.jsonl");

    // A dry run reports the leases and writes nothing.
    let dry = gwi(home.path(), &["import", "--dry-run"]);
    assert!(dry.status.success());
    assert!(String::from_utf8_lossy(&dry.stdout).contains("lease lease-live"));
    assert!(!target.exists());

    let imported = gwi(home.path(), &["import"]);
    assert!(imported.status.success());
    let report = String::from_utf8_lossy(&imported.stdout);
    assert!(report.contains("added       lease lease-live"), "{report}");
    assert!(
        report.contains("added       lease lease-expired"),
        "{report}"
    );
    assert!(report.contains("warning: 1 live lease(s)"), "{report}");
    let copied = std::fs::read_to_string(&target).unwrap();
    assert!(copied.contains("lease-live") && copied.contains("lease-expired"));

    let ledger_after_first = std::fs::read(&target).unwrap();
    let again = gwi(home.path(), &["import"]);
    assert!(again.status.success());
    assert!(String::from_utf8_lossy(&again.stdout)
        .contains("0 added, 0 overwritten, 0 released, 2 unchanged, 0 conflict(s)"));
    assert_eq!(std::fs::read(&target).unwrap(), ledger_after_first);
    assert_eq!(std::fs::read(&source).unwrap(), source_before);
}

#[test]
fn import_propagates_a_release_made_in_omni_dev_after_the_import() {
    let home = tempfile::tempdir().unwrap();
    write_omni_dev_settings(home.path());
    let source = write_omni_dev_ledger(home.path());
    let target = state_dir(home.path())
        .join("gwi")
        .join("lease-ledger.jsonl");
    assert!(gwi(home.path(), &["import"]).status.success());
    assert!(!std::fs::read_to_string(&target)
        .unwrap()
        .contains("released_at"));

    // omni-dev writes under the live lease, then releases it after the import.
    let released = std::fs::read_to_string(&source)
        .unwrap()
        .lines()
        .map(|line| {
            let mut row: serde_json::Value = serde_json::from_str(line).unwrap();
            if row["token"] == "lease-live" {
                row["version"] = serde_json::json!("5");
                row["modified_time"] = serde_json::json!("2026-10-01T00:00:00Z");
                row["released_at"] = serde_json::json!("2026-10-01T00:00:00Z");
            }
            row.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&source, &released).unwrap();

    let again = gwi(home.path(), &["import"]);
    assert!(again.status.success());
    let report = String::from_utf8_lossy(&again.stdout);
    assert!(report.contains("released    lease lease-live"), "{report}");
    assert!(
        report.contains("0 added, 0 overwritten, 1 released, 1 unchanged, 0 conflict(s)"),
        "{report}"
    );
    assert!(std::fs::read_to_string(&target)
        .unwrap()
        .contains("2026-10-01T00:00:00Z"));

    let target_rows = std::fs::read_to_string(&target).unwrap();
    for line in target_rows.lines() {
        let row: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(row["version"], "4");
        assert!(row.get("modified_time").is_none());
    }
    let ledger_after_release = std::fs::read(&target).unwrap();
    let third = gwi(home.path(), &["import"]);
    assert!(third.status.success());
    assert!(String::from_utf8_lossy(&third.stdout)
        .contains("0 added, 0 overwritten, 0 released, 2 unchanged, 0 conflict(s)"));
    assert_eq!(std::fs::read(&target).unwrap(), ledger_after_release);
    assert_eq!(std::fs::read_to_string(&source).unwrap(), released);
}

#[test]
fn import_reads_the_ledger_from_source_ledger_and_fails_on_a_ledger_conflict() {
    let home = tempfile::tempdir().unwrap();
    write_omni_dev_settings(home.path());
    let elsewhere = home.path().join("elsewhere.jsonl");
    std::fs::write(
        &elsewhere,
        r#"{"token":"t1","file_id":"f","version":"1","backup":{"kind":"drive_copy","file_id":"c"},"acquired_at":"2020-01-01T00:00:00Z","expires_at":"2020-01-01T00:15:00Z"}"#,
    )
    .unwrap();
    let source_ledger = elsewhere.to_str().unwrap();
    let target = state_dir(home.path())
        .join("gwi")
        .join("lease-ledger.jsonl");

    assert!(
        gwi(home.path(), &["import", "--source-ledger", source_ledger])
            .status
            .success()
    );
    std::fs::write(
        &elsewhere,
        std::fs::read_to_string(&elsewhere)
            .unwrap()
            .replace("\"1\"", "\"2\""),
    )
    .unwrap();

    let refused = gwi(home.path(), &["import", "--source-ledger", source_ledger]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--force"));
    assert!(std::fs::read_to_string(&target).unwrap().contains("\"1\""));

    let forced = gwi(
        home.path(),
        &["import", "--source-ledger", source_ledger, "--force"],
    );
    assert!(forced.status.success());
    assert!(std::fs::read_to_string(&target).unwrap().contains("\"2\""));
}

#[test]
fn import_without_a_source_file_says_how_to_find_one() {
    let home = tempfile::tempdir().unwrap();

    let output = gwi(home.path(), &["import"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--source"));
}

/// A request-log line: an HTTP record and a Drive mutation, both well in the past.
const HTTP_LINE: &str = r#"{"id":"rec-1","invocation_id":"inv-1","kind":"http","timestamp":"2020-01-01T00:00:00.000Z","service":"gmail","method":"GET","status_code":200,"url":"https://gmail.googleapis.com/gmail/v1/users/me/messages"}"#;
const DRIVE_MUTATION_LINE: &str = r#"{"id":"rec-2","invocation_id":"inv-2","kind":"drivemutation","timestamp":"2020-01-01T00:00:01.000Z","command":["drive","move"],"service":"drive","context":{"file_id":"f1","file_name":"report.pdf","status":"blocked"}}"#;
/// An audit-log line.
const AUDIT_LINE: &str = r#"{"id":"rec-3","invocation_id":"inv-2","kind":"audit","timestamp":"2020-01-01T00:00:02.000Z","command":["drive","lease-acquire"],"context":{"integration":"drive","lease_id":"lease-1","verdict":"acquired"}}"#;

fn write_logs(home: &Path) {
    std::fs::write(
        home.join("log.jsonl"),
        format!("{HTTP_LINE}\n{DRIVE_MUTATION_LINE}\n"),
    )
    .unwrap();
    std::fs::write(home.join("audit.jsonl"), format!("{AUDIT_LINE}\n")).unwrap();
}

#[test]
fn log_warns_once_when_request_log_resolves_to_audit_log() {
    let home = tempfile::tempdir().unwrap();
    let audit = home.path().join("audit.jsonl");
    let contents = format!("{AUDIT_LINE}\n");
    std::fs::write(&audit, &contents).unwrap();

    // Each child resolves the path for the command and again for its invocation record.
    // A fresh process must emit its own warning rather than suppressing it globally.
    for _ in 0..2 {
        let mut command = Command::new(env!("CARGO_BIN_EXE_gwi"));
        let output = common::scrub_ambient_env(&mut command)
            .arg("log")
            .env("HOME", home.path())
            .env("GWI_LOG_FILE", &audit)
            .env("GWI_AUDIT_LOG_FILE", &audit)
            .env("GWI_LOG_DISABLE", "0")
            .env("RUST_LOG", "warn")
            .output()
            .unwrap();

        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            stderr
                .matches("GWI_LOG_FILE resolves to the audit log")
                .count(),
            1,
            "{stderr}"
        );
        assert!(
            stderr.contains("refusing to use it as the request log path"),
            "{stderr}"
        );
        assert!(
            stderr.contains("could not resolve the log file path"),
            "{stderr}"
        );
        assert!(output.stdout.is_empty());
        assert_eq!(std::fs::read_to_string(&audit).unwrap(), contents);
    }
}

#[test]
fn log_reads_the_request_log_and_audit_reads_the_audit_log() {
    let home = tempfile::tempdir().unwrap();
    write_logs(home.path());

    let requests = gwi(home.path(), &["log", "-o", "json"]);
    assert!(requests.status.success());
    let stdout = String::from_utf8_lossy(&requests.stdout);
    assert_eq!(stdout, format!("{HTTP_LINE}\n{DRIVE_MUTATION_LINE}\n"));

    let audit = gwi(home.path(), &["log", "--audit", "-o", "json"]);
    assert!(audit.status.success());
    let stdout = String::from_utf8_lossy(&audit.stdout);
    assert_eq!(stdout, format!("{AUDIT_LINE}\n"));
}

#[test]
fn log_rotated_searches_backups_in_numeric_order_with_one_limit() {
    let home = tempfile::tempdir().unwrap();
    for (suffix, line) in [
        (".10", HTTP_LINE),
        (".2", DRIVE_MUTATION_LINE),
        (".1", AUDIT_LINE),
        ("", HTTP_LINE),
    ] {
        std::fs::write(
            home.path().join(format!("log.jsonl{suffix}")),
            format!("{line}\n"),
        )
        .unwrap();
    }
    // These are not files named by the rotation writer.
    for suffix in [".0", ".01", ".-1", ".tmp", ".2.tmp", ".4294967296"] {
        std::fs::write(home.path().join(format!("log.jsonl{suffix}")), "bad\n").unwrap();
    }
    std::fs::create_dir(home.path().join("log.jsonl.3")).unwrap();
    let default = gwi(home.path(), &["log", "-o", "json"]);
    assert!(default.status.success());
    assert_eq!(default.stdout, format!("{HTTP_LINE}\n").as_bytes());
    let all = gwi(home.path(), &["log", "--rotated", "-o", "json"]);
    assert!(all.status.success(), "{all:?}");
    assert_eq!(
        all.stdout,
        format!("{HTTP_LINE}\n{DRIVE_MUTATION_LINE}\n{AUDIT_LINE}\n{HTTP_LINE}\n").as_bytes()
    );
    assert!(all.stderr.is_empty(), "{all:?}");
    let recent = gwi(home.path(), &["log", "--rotated", "-o", "json", "-n", "3"]);
    assert!(recent.status.success());
    assert_eq!(
        recent.stdout,
        format!("{DRIVE_MUTATION_LINE}\n{AUDIT_LINE}\n{HTTP_LINE}\n").as_bytes()
    );
    let filtered = gwi(
        home.path(),
        &[
            "log",
            "--rotated",
            "-o",
            "json",
            "--query",
            "kind:drivemutation",
            "--query",
            "file_id:f1",
            "--status",
            "blocked",
            "-n",
            "1",
        ],
    );
    assert!(filtered.status.success());
    assert_eq!(
        filtered.stdout,
        format!("{DRIVE_MUTATION_LINE}\n").as_bytes()
    );
    // Observations include archives, so their fields/statuses produce no typo warnings.
    assert!(filtered.stderr.is_empty(), "{filtered:?}");
    let zero = gwi(home.path(), &["log", "--rotated", "-n", "0"]);
    assert!(zero.status.success());
    assert!(zero.stdout.is_empty());
    let audit = gwi(home.path(), &["log", "--rotated", "--audit"]);
    assert_eq!(audit.status.code(), Some(2));
}

#[test]
fn log_rotated_finds_archived_records_without_a_live_file_and_warns_per_file() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("log.jsonl.2"),
        format!("bad\n{DRIVE_MUTATION_LINE}\n"),
    )
    .unwrap();
    // An archived final record is read even without a terminating newline.
    std::fs::write(home.path().join("log.jsonl.1"), HTTP_LINE).unwrap();
    let default = gwi(home.path(), &["log", "-o", "json"]);
    assert!(default.status.success());
    assert!(default.stdout.is_empty());
    let archived = gwi(
        home.path(),
        &["log", "--rotated", "-o", "json", "--status", "blocked"],
    );
    assert!(archived.status.success());
    assert_eq!(
        archived.stdout,
        format!("{DRIVE_MUTATION_LINE}\n").as_bytes()
    );
    let warning = String::from_utf8_lossy(&archived.stderr);
    assert!(warning.contains("1 unparseable line"), "{warning}");
    assert!(warning.contains("log.jsonl.2"), "{warning}");
    assert!(!warning.contains("no drivemutation"), "{warning}");
    let both = gwi(home.path(), &["log", "--rotated", "-o", "json"]);
    assert!(both.status.success());
    assert_eq!(
        both.stdout,
        format!("{DRIVE_MUTATION_LINE}\n{HTTP_LINE}\n").as_bytes()
    );
}

#[test]
fn log_query_selects_drive_mutations() {
    let home = tempfile::tempdir().unwrap();
    write_logs(home.path());

    let output = gwi(home.path(), &["log", "--query", "kind:drivemutation"]);

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    assert!(stdout.contains("drive move"), "{stdout}");
    assert!(stdout.contains("report.pdf"), "{stdout}");
}

#[test]
fn log_query_warns_on_a_field_no_record_has_but_still_exits_zero() {
    let home = tempfile::tempdir().unwrap();
    write_logs(home.path());

    let typo = gwi(home.path(), &["log", "--query", "servce:drive"]);
    assert!(typo.status.success());
    assert_eq!(typo.stdout, b"");
    let stderr = String::from_utf8_lossy(&typo.stderr);
    assert!(stderr.contains("`servce`"), "{stderr}");
    assert!(stderr.contains("Did you mean `service`?"), "{stderr}");

    // A real context key, a built-in field and a quoted literal stay quiet.
    for query in ["file_id:f1", "service:drive", "\"12:34:56\""] {
        let output = gwi(home.path(), &["log", "--query", query]);
        assert!(output.status.success(), "{query}");
        assert_eq!(output.stderr, b"", "{query}");
    }
}

#[test]
fn log_recovers_a_record_after_a_partial_prefix() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("log.jsonl"),
        format!("{{partial{DRIVE_MUTATION_LINE}\n"),
    )
    .unwrap();
    let output = gwi(home.path(), &["log", "-o", "json"]);
    assert!(output.status.success());
    assert_eq!(output.stdout, format!("{DRIVE_MUTATION_LINE}\n").as_bytes());
    assert!(output.stderr.is_empty());
}

#[test]
fn log_follow_recovers_appended_records_and_warns_on_stderr() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::Duration;

    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("log.jsonl");
    std::fs::write(&path, format!("{HTTP_LINE}\n{{partial")).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_gwi"));
    let mut child = common::scrub_ambient_env(&mut command)
        .args(["log", "--follow", "-o", "json"])
        .env("HOME", home.path())
        .env("GWI_LOG_FILE", &path)
        .env("GWI_LOG_DISABLE", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (out_tx, out_rx) = mpsc::channel();
    let out_reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if out_tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let (err_tx, err_rx) = mpsc::channel();
    let err_reader = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            if err_tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let timeout = Duration::from_secs(10);
    let initial = out_rx.recv_timeout(timeout);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(file, "{DRIVE_MUTATION_LINE}\njunk").unwrap();
    let recovered = out_rx.recv_timeout(timeout);
    let warning = err_rx.recv_timeout(timeout);
    // Reap before assertions so even a failed timeout cannot leave a follower running.
    child.kill().unwrap();
    child.wait().unwrap();
    out_reader.join().unwrap();
    err_reader.join().unwrap();
    assert_eq!(initial.unwrap(), HTTP_LINE);
    assert_eq!(recovered.unwrap(), DRIVE_MUTATION_LINE);
    assert_eq!(
        warning.unwrap(),
        format!("warning: skipped 1 unparseable line in {}", path.display())
    );
    assert!(out_rx.try_recv().is_err());
    assert!(err_rx.try_recv().is_err());
}

#[test]
fn log_warns_about_unparseable_lines_but_leaves_stdout_and_exit_code_alone() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("log.jsonl");
    // Two corrupt lines among good ones, plus a blank line and a trailing partial line.
    std::fs::write(
        &path,
        format!("not json\n{HTTP_LINE}\n\n{{\"id\":\n{DRIVE_MUTATION_LINE}\n{{\"id\":\"3\",\"ki"),
    )
    .unwrap();

    let output = gwi(home.path(), &["log", "-o", "json"]);

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{HTTP_LINE}\n{DRIVE_MUTATION_LINE}\n")
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!(
            "warning: skipped 3 unparseable lines in {}\n",
            path.display()
        )
    );

    // A clean log, blank lines and a complete unterminated record stay quiet.
    std::fs::write(&path, format!("{HTTP_LINE}\n\n{DRIVE_MUTATION_LINE}")).unwrap();
    let output = gwi(home.path(), &["log", "-o", "json"]);
    assert!(output.status.success());
    assert_eq!(output.stderr, b"");
}

#[test]
fn log_warns_on_a_status_word_no_drive_mutation_has_but_still_exits_zero() {
    let home = tempfile::tempdir().unwrap();
    write_logs(home.path());

    for args in [
        ["log", "--status", "blokced"],
        ["log", "--query", "status:blokced"],
    ] {
        let typo = gwi(home.path(), &args);
        assert!(typo.status.success(), "{args:?}");
        assert_eq!(typo.stdout, b"", "{args:?}");
        let stderr = String::from_utf8_lossy(&typo.stderr);
        assert!(stderr.contains("`blokced`"), "{stderr}");
        assert!(stderr.contains("Did you mean `blocked`?"), "{stderr}");
    }

    // A status that is present, and a numeric status, stay quiet.
    for args in [
        ["log", "--status", "blocked"],
        ["log", "--query", "status:blocked"],
        ["log", "--status", "5xx"],
    ] {
        let output = gwi(home.path(), &args);
        assert!(output.status.success(), "{args:?}");
        assert_eq!(output.stderr, b"", "{args:?}");
    }

    // A valid word no record has still says so, without a suggestion.
    let absent = gwi(home.path(), &["log", "--status", "written"]);
    assert!(absent.status.success());
    let stderr = String::from_utf8_lossy(&absent.stderr);
    assert!(stderr.contains("`written`"), "{stderr}");
    assert!(!stderr.contains("Did you mean"), "{stderr}");

    // With no `drivemutation` record in the log there is nothing to judge by.
    std::fs::write(home.path().join("log.jsonl"), format!("{HTTP_LINE}\n")).unwrap();
    let output = gwi(home.path(), &["log", "--status", "blokced"]);
    assert!(output.status.success());
    assert_eq!(output.stderr, b"");
}

#[test]
fn log_prints_field_and_status_warnings_together_even_with_a_limit() {
    let home = tempfile::tempdir().unwrap();
    write_logs(home.path());

    let output = gwi(
        home.path(),
        &[
            "log",
            "--limit",
            "1",
            "--query",
            "servce:drive status:blokced",
        ],
    );
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Did you mean `service`?"), "{stderr}");
    assert!(stderr.contains("Did you mean `blocked`?"), "{stderr}");
}

#[test]
fn log_query_quoted_not_is_a_literal_and_status_accepts_drive_statuses() {
    let home = tempfile::tempdir().unwrap();
    write_logs(home.path());

    // `report.pdf` is in the file name; the quoted `"not"` is not an operator.
    let output = gwi(home.path(), &["log", "--query", "report \"not\""]);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"", "no record contains the word `not`");
    let output = gwi(home.path(), &["log", "--query", "report NOT gmail"]);
    assert_eq!(String::from_utf8_lossy(&output.stdout).lines().count(), 1);

    let flag = gwi(home.path(), &["log", "--status", "blocked"]);
    let query = gwi(home.path(), &["log", "--query", "status:blocked"]);
    assert!(flag.status.success());
    assert_eq!(flag.stdout, query.stdout);
    assert!(String::from_utf8_lossy(&flag.stdout).contains("drive move"));
}

#[test]
fn log_rejects_malformed_statuses_before_scanning_an_empty_log() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("log.jsonl"), "").unwrap();

    for (spec, bad_value) in [
        ("9xx", "9xx"),
        (">=abc", ">=abc"),
        ("20x", "20x"),
        ("blocked,4xx", "4xx"),
        ("4xx,blocked", "blocked"),
        ("blocked,>=400", ">=400"),
        (">=400,blocked", ">=400,blocked"),
    ] {
        let query = format!("status:{spec}");
        for (flag, value) in [("--status", spec), ("--query", query.as_str())] {
            let output = gwi(home.path(), &["log", flag, value]);
            assert!(!output.status.success(), "{flag} {value}");
            assert!(output.stdout.is_empty(), "{flag} {value}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains(bad_value), "{flag} {value}: {stderr}");
            if flag == "--query" {
                assert!(stderr.contains("invalid --query"), "{stderr}");
            }
        }
    }
}

#[test]
fn log_prune_trims_the_request_log_but_never_the_audit_log() {
    let home = tempfile::tempdir().unwrap();
    write_logs(home.path());

    let output = gwi(home.path(), &["log", "prune", "--older-than", "1d"]);

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Removed 2 record(s)"));
    assert_eq!(
        std::fs::read_to_string(home.path().join("log.jsonl")).unwrap(),
        ""
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join("audit.jsonl")).unwrap(),
        format!("{AUDIT_LINE}\n")
    );

    // `--audit` is refused rather than ignored.
    let refused = gwi(
        home.path(),
        &["log", "prune", "--audit", "--older-than", "1d"],
    );
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("does not support --audit"));
    assert_eq!(
        std::fs::read_to_string(home.path().join("audit.jsonl")).unwrap(),
        format!("{AUDIT_LINE}\n")
    );
}

#[test]
fn log_follow_exits_when_its_output_pipe_closes() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let home = tempfile::tempdir().unwrap();
    // Far more than a pipe buffer holds, so the child is still writing the backlog
    // when the reader goes away and its next write fails with `EPIPE`.
    std::fs::write(
        home.path().join("log.jsonl"),
        format!("{HTTP_LINE}\n").repeat(5000),
    )
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_gwi"));
    let mut child = common::scrub_ambient_env(&mut child)
        .args(["log", "--follow", "-o", "json"])
        .env("HOME", home.path())
        .env("GWI_LOG_FILE", home.path().join("log.jsonl"))
        .env("GWI_AUDIT_LOG_FILE", home.path().join("audit.jsonl"))
        .env("GWI_LOG_DISABLE", "1")
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to run the gwi binary");

    // Like `| head -1`: read one line, then close the pipe.
    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut first)
        .unwrap();
    assert_eq!(first.trim_end(), HTTP_LINE);

    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("gwi log --follow kept running after its output pipe closed");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "{status}");
}

/// An idle follow writes nothing, so it can't learn from a failed write that its
/// reader has gone; it has to notice the hangup itself.
#[cfg(unix)]
#[test]
fn log_follow_exits_when_its_output_pipe_closes_while_idle() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let home = tempfile::tempdir().unwrap();
    // Small enough for the pipe buffer, so no write fails, and nothing is appended.
    std::fs::write(home.path().join("log.jsonl"), format!("{HTTP_LINE}\n")).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_gwi"))
        .args(["log", "--follow", "-o", "json"])
        .env("HOME", home.path())
        .env("GWI_LOG_FILE", home.path().join("log.jsonl"))
        .env("GWI_AUDIT_LOG_FILE", home.path().join("audit.jsonl"))
        .env("GWI_LOG_DISABLE", "1")
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to run the gwi binary");

    // Like `| head -1`: read the one line, then close the pipe.
    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut first)
        .unwrap();
    assert_eq!(first.trim_end(), HTTP_LINE);

    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("an idle gwi log --follow kept running after its output pipe closed");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "{status}");
}
