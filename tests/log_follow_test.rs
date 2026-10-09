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
    common::scrub_ambient_env(&mut cmd)
        .env("HOME", home)
        .env("GWI_LOG_FILE", home.join("log.jsonl"))
        .env("GWI_AUDIT_LOG_FILE", home.join("audit.jsonl"))
        .env("GWI_LOG_DISABLE", "1");
    cmd
}

struct Follow {
    child: Child,
    lines: Receiver<String>,
    reader: Option<JoinHandle<()>>,
}

impl Follow {
    fn start(home: &Path) -> Self {
        let mut child = command(home)
            .args(["log", "--follow", "-o", "json"])
            .stdout(Stdio::piped())
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
        Self {
            child,
            lines,
            reader: Some(reader),
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
