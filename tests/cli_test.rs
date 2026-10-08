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
        .env("GWI_AUDIT_LOG_FILE", home.join("audit.jsonl"))
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
        .contains("0 added, 0 overwritten, 2 unchanged, 0 conflict(s)"));
    assert_eq!(std::fs::read(&target).unwrap(), ledger_after_first);
    assert_eq!(std::fs::read(&source).unwrap(), source_before);
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

    let mut child = Command::new(env!("CARGO_BIN_EXE_gwi"))
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
