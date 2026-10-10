//! Real prune/follow concurrency tests, also run by the focused Windows CI job.

#![cfg(any(unix, windows))]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

fn record(id: &str) -> String {
    serde_json::json!({
        "id": id,
        "kind": "http",
        "timestamp": "2020-01-01T00:00:00Z",
        "service": "gmail",
        "method": "GET",
        "status_code": 200,
        "url": "https://example.invalid/"
    })
    .to_string()
}

fn command(home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gwi"));
    common::scrub_ambient_env(&mut cmd).env("HOME", home);
    common::pin_log_env(&mut cmd, home);
    cmd
}

struct Follow {
    child: Child,
    lines: Receiver<String>,
    reader: Option<JoinHandle<()>>,
    warnings: Receiver<String>,
    error_reader: Option<JoinHandle<()>>,
}

impl Follow {
    fn start(home: &Path) -> Self {
        Self::start_with_args(home, &[])
    }

    fn start_with_args(home: &Path, args: &[&str]) -> Self {
        let mut child = command(home)
            .args(["log", "--follow", "-o", "json"])
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if sender.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        let stderr = child.stderr.take().unwrap();
        let (sender, warnings) = mpsc::channel();
        let error_reader = thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                if sender.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            lines,
            reader: Some(reader),
            warnings,
            error_reader: Some(error_reader),
        }
    }

    fn expect_line(&self, expected: &str) {
        assert_eq!(
            self.lines.recv_timeout(Duration::from_secs(20)).unwrap(),
            expected
        );
    }

    fn expect_quiet(&mut self) {
        // Four polling intervals: detect the replacement before the next append.
        assert_eq!(
            self.lines.recv_timeout(Duration::from_secs(1)),
            Err(RecvTimeoutError::Timeout),
            "replacement contents must not replay"
        );
        assert!(self.child.try_wait().unwrap().is_none(), "follow exited");
    }
}

impl Drop for Follow {
    fn drop(&mut self) {
        // Windows cannot detect an idle closed stdout pipe. Explicit cleanup also
        // reaps the child and releases its log handles when an assertion fails.
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(reader) = self.error_reader.take() {
            let _ = reader.join();
        }
    }
}

fn prune_under_follow(larger: bool) {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("log.jsonl");
    let first = record("startup");
    let retained = record(if larger {
        "a-large-retained-record-with-a-longer-id-than-the-startup-record"
    } else {
        "retained"
    });
    // An unterminated record is not consumed by follow, but prune will retain
    // and newline-terminate it. This makes the replacement exceed the saved
    // offset without racing an append against follow's next poll.
    let initial = format!("{first}\n{retained}{}", if larger { "" } else { "\n" });
    std::fs::write(&path, &initial).unwrap();
    let mut follow = Follow::start(home.path());
    follow.expect_line(&first);
    if !larger {
        follow.expect_line(&retained);
    }
    follow.expect_quiet();
    let saved_offset = if larger {
        first.len() + 1
    } else {
        initial.len()
    };

    let output = command(home.path())
        .args([
            "log",
            "prune",
            "--max-size",
            &(retained.len() + 1).to_string(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "prune failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Removed 1 record(s); kept 1"));
    let replacement = std::fs::read_to_string(&path).unwrap();
    assert_eq!(replacement, format!("{retained}\n"));
    if larger {
        assert!(replacement.len() > saved_offset);
    } else {
        assert!(replacement.len() < saved_offset);
    }
    follow.expect_quiet();

    let marker = record("appended-after-prune");
    {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{marker}").unwrap();
    }
    follow.expect_line(&marker);
    follow.expect_quiet();
}

#[test]
fn log_prune_under_live_follow_with_a_larger_replacement() {
    prune_under_follow(true);
}

#[test]
fn log_prune_under_live_follow_with_a_smaller_replacement() {
    prune_under_follow(false);
}

fn append(path: &Path, raw: &str) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{raw}").unwrap();
}

#[test]
fn follow_warns_once_after_missing_or_empty_backlog() {
    for missing in [true, false] {
        for args in [
            vec!["--query", "servce:drive"],
            vec!["--status", "blokced"],
            vec!["--query", "status:blokced"],
        ] {
            let home = tempfile::tempdir().unwrap();
            let path = home.path().join("log.jsonl");
            if !missing {
                std::fs::write(&path, "").unwrap();
            }
            let mut follow = Follow::start_with_args(home.path(), &args);
            assert_eq!(
                follow.warnings.recv_timeout(Duration::from_secs(1)),
                Err(RecvTimeoutError::Timeout)
            );
            let mutation = serde_json::json!({
                "id": "first", "kind": "drivemutation",
                "timestamp": "2020-01-01T00:00:00Z",
                "context": {"status": "blocked"}
            })
            .to_string();
            append(&path, &mutation);
            let warning = follow
                .warnings
                .recv_timeout(Duration::from_secs(20))
                .unwrap();
            assert!(
                warning.contains(if args[1] == "servce:drive" {
                    "`servce`"
                } else {
                    "`blokced`"
                }),
                "{warning}"
            );
            assert!(warning.contains("so far"), "{warning}");
            assert!(warning.contains("Did you mean"), "{warning}");
            append(&path, &mutation);
            assert_eq!(
                follow.warnings.recv_timeout(Duration::from_secs(1)),
                Err(RecvTimeoutError::Timeout)
            );
            assert!(follow.child.try_wait().unwrap().is_none());
            assert_eq!(follow.lines.try_recv(), Err(mpsc::TryRecvError::Empty));
        }
    }
}

#[test]
fn follow_status_warning_waits_for_mutations_after_http_backlog() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("log.jsonl");
    append(&path, &record("http-backlog"));
    let follow = Follow::start_with_args(home.path(), &["--status", "blokced"]);
    assert_eq!(
        follow.warnings.recv_timeout(Duration::from_secs(1)),
        Err(RecvTimeoutError::Timeout)
    );
    append(&path, &record("http-appended"));
    assert_eq!(
        follow.warnings.recv_timeout(Duration::from_secs(1)),
        Err(RecvTimeoutError::Timeout)
    );
    append(
        &path,
        &serde_json::json!({
            "id": "mutation", "kind": "drivemutation",
            "timestamp": "2020-01-01T00:00:00Z",
            "context": {"status": "blocked"}
        })
        .to_string(),
    );
    let warning = follow
        .warnings
        .recv_timeout(Duration::from_secs(20))
        .unwrap();
    assert!(
        warning.contains("the 1 drivemutation record scanned"),
        "{warning}"
    );
    assert!(warning.contains("`blokced`"), "{warning}");
}

#[test]
fn log_follow_batches_corrupt_appends_and_waits_for_partial_lines() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("log.jsonl");
    let first = record("startup");
    std::fs::write(&path, format!("{first}\n")).unwrap();
    let mut follow = Follow::start(home.path());
    follow.expect_line(&first);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();

    // One write makes the burst available together for the next drain.
    let marker = record("after-corruption");
    file.write_all(format!("broken\ninvalid\n\n \t\n{marker}\n").as_bytes())
        .unwrap();
    follow.expect_line(&marker);
    assert_eq!(
        follow
            .warnings
            .recv_timeout(Duration::from_secs(20))
            .unwrap(),
        format!(
            "warning: skipped 2 unparseable lines in {} (lines 2, 3)",
            path.display()
        )
    );
    assert_eq!(
        follow.warnings.recv_timeout(Duration::from_secs(1)),
        Err(RecvTimeoutError::Timeout)
    );
    follow.expect_quiet();

    // A valid record completed later is emitted exactly once, with no warning.
    let later = record("completed-later");
    let split = later.len() / 2;
    file.write_all(&later.as_bytes()[..split]).unwrap();
    follow.expect_quiet();
    assert_eq!(follow.warnings.try_recv(), Err(mpsc::TryRecvError::Empty));
    file.write_all(format!("{}\n", &later[split..]).as_bytes())
        .unwrap();
    follow.expect_line(&later);
    follow.expect_quiet();
    assert_eq!(follow.warnings.try_recv(), Err(mpsc::TryRecvError::Empty));

    file.write_all(b"unfinished").unwrap();
    follow.expect_quiet();
    assert_eq!(follow.warnings.try_recv(), Err(mpsc::TryRecvError::Empty));
    file.write_all(b"\n").unwrap();
    assert_eq!(
        follow
            .warnings
            .recv_timeout(Duration::from_secs(20))
            .unwrap(),
        format!(
            "warning: skipped 1 unparseable line in {} (lines 8)",
            path.display()
        )
    );
    follow.expect_quiet();
    assert_eq!(follow.warnings.try_recv(), Err(mpsc::TryRecvError::Empty));
}
