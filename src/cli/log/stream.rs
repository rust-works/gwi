//! Streaming reader: filter and render the log line by line.
//!
//! The backlog is read without buffering the whole file (a `--limit` keeps only
//! the most recent N matches in a ring buffer); `--follow` then tails newly
//! appended complete lines. A broken pipe (e.g. piping into `head`) is treated
//! as a clean exit, not an error, and ends the scan at once. A reader that goes
//! away while `--follow` is idle is only noticed on the next write.
//!
//! `--follow` also notices the log being replaced (`gwi log prune`, rotation):
//! on unix by the file's device and inode, elsewhere only by its shrinking.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::Result;

use super::format;
use super::query::Filter;
use super::Format;
use crate::request_log::LogRecord;

/// Poll interval while following the log.
const FOLLOW_POLL: Duration = Duration::from_millis(250);

/// Identity of a file on disk: `(device, inode)` on unix; always `None` elsewhere,
/// where only shrinkage reveals a replacement.
type FileId = Option<(u64, u64)>;

#[cfg(unix)]
fn file_id(meta: &std::fs::Metadata) -> FileId {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn file_id(_meta: &std::fs::Metadata) -> FileId {
    None
}

/// Where `--follow` has got to in the file it is tailing.
#[derive(Clone, Copy, Debug)]
struct Tail {
    /// Byte offset just past the last complete line read.
    pos: u64,
    /// Identity of the file `pos` refers to, when known.
    id: FileId,
}

/// How the backlog scan ended.
#[derive(Debug, PartialEq, Eq)]
enum Backlog {
    /// Every line was read; the offset is just past the last newline-terminated one.
    Complete(u64),
    /// The output pipe closed (e.g. `| head -1`), so the scan stopped early.
    ReaderGone,
}

/// Streams the log file at `path`, applying `filter` and rendering as `format`.
pub fn run(
    path: &Path,
    filter: &Filter,
    format: Format,
    limit: Option<usize>,
    follow: bool,
) -> Result<()> {
    let stdout = io::stdout();
    let mut out = stdout.lock();

    let mut tail = Tail { pos: 0, id: None };
    match File::open(path) {
        Ok(file) => {
            // Taken from the handle that is scanned, so a replacement after this
            // point is seen as a change of identity by the follow loop.
            tail.id = file.metadata().ok().and_then(|m| file_id(&m));
            let mut reader = BufReader::new(file);
            match emit_backlog(&mut reader, filter, format, limit, &mut out)? {
                Backlog::Complete(pos) => tail.pos = pos,
                Backlog::ReaderGone => return Ok(()),
            }
            warn_unknown_fields(filter);
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if !follow {
                return Ok(());
            }
        }
        Err(e) => return Err(e.into()),
    }

    if follow {
        if let Err(e) = follow_loop(path, filter, format, tail, &mut out) {
            return swallow_broken_pipe(e);
        }
    }
    Ok(())
}

/// Prints the filter's unknown-field warnings to stderr, once the backlog has
/// been scanned: a query field that is no built-in name and appears in no
/// record's `context` would otherwise just match nothing. Stderr only, so the
/// output stays machine-readable and the exit code is unchanged.
fn warn_unknown_fields(filter: &Filter) {
    let mut err = io::stderr().lock();
    for warning in filter.unknown_field_warnings() {
        // Best effort: a closed stderr must not fail the search.
        let _ = writeln!(err, "{warning}");
    }
}

/// Reads the next line (through its `\n`, if any) into `line`, returning the byte
/// count. Invalid UTF-8 is replaced rather than failing the read, so one corrupt
/// line cannot abort the scan; such a line is then skipped by [`render_if_match`]
/// if it no longer parses, as any other malformed line is.
fn read_line_lossy<R: BufRead>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    line: &mut String,
) -> io::Result<usize> {
    buf.clear();
    let n = reader.read_until(b'\n', buf)?;
    line.clear();
    line.push_str(&String::from_utf8_lossy(buf));
    Ok(n)
}

/// Reads every existing line, emitting matches. With `limit`, only the most
/// recent N matches are kept (ring buffer) and printed at the end; without it,
/// matches stream out as they are read. Returns the byte offset just past the last
/// newline-terminated line: a trailing partial line (a writer mid-append) is
/// still tried, but is not counted, so `--follow` re-reads it once complete.
/// Stops at the first write to a closed pipe, without reading the rest.
fn emit_backlog<R: BufRead, W: Write>(
    reader: &mut R,
    filter: &Filter,
    format: Format,
    limit: Option<usize>,
    out: &mut W,
) -> Result<Backlog> {
    let mut pos = 0u64;
    let mut ring: VecDeque<String> = VecDeque::new();
    let mut buf = Vec::new();
    let mut line = String::new();
    loop {
        let n = read_line_lossy(reader, &mut buf, &mut line)?;
        if n == 0 {
            break;
        }
        if line.ends_with('\n') {
            pos += n as u64;
        }
        if let Some(rendered) = render_if_match(&line, filter, format) {
            match limit {
                Some(cap) => {
                    ring.push_back(rendered);
                    while ring.len() > cap {
                        ring.pop_front();
                    }
                }
                None => {
                    if !write_line(out, &rendered)? {
                        return Ok(Backlog::ReaderGone);
                    }
                }
            }
        }
    }
    for rendered in &ring {
        if !write_line(out, rendered)? {
            return Ok(Backlog::ReaderGone);
        }
    }
    Ok(Backlog::Complete(pos))
}

/// Writes one rendered record. `Ok(false)` means the reader closed the pipe and the
/// caller should stop; any other write error propagates.
fn write_line<W: Write>(out: &mut W, rendered: &str) -> io::Result<bool> {
    match writeln!(out, "{rendered}") {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(e) => Err(e),
    }
}

/// Tails the file from `pos`, printing newly appended complete lines forever
/// (until the process is interrupted or a write finds the pipe closed). Restarts
/// from the top when the file is truncated or replaced.
fn follow_loop<W: Write>(
    path: &Path,
    filter: &Filter,
    format: Format,
    mut tail: Tail,
    out: &mut W,
) -> Result<()> {
    loop {
        drain_appended(path, filter, format, &mut tail, out)?;
        std::thread::sleep(FOLLOW_POLL);
    }
}

/// Reads and emits any complete lines appended past `tail.pos`, advancing the
/// tail. Restarts from the top if the file was replaced (its identity changed)
/// or shrank (truncation); a no-op if the file is absent or has not grown.
/// A trailing partial line (no newline yet) is left for the next call.
fn drain_appended<W: Write>(
    path: &Path,
    filter: &Filter,
    format: Format,
    tail: &mut Tail,
    out: &mut W,
) -> Result<()> {
    let Ok(file) = File::open(path) else {
        return Ok(());
    };
    // Identity and length come from the handle that is read, not from the path.
    let meta = file.metadata().ok();
    // A failed `fstat` says nothing about replacement, so keep the saved identity.
    let id = meta.as_ref().map_or(tail.id, file_id);
    let len = meta.map_or(tail.pos, |m| m.len());
    if id != tail.id || len < tail.pos {
        tail.pos = 0; // replaced, truncated or rotated — restart
    }
    tail.id = id;
    if len > tail.pos {
        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::Start(tail.pos))?;
        let mut buf = Vec::new();
        let mut line = String::new();
        loop {
            let n = read_line_lossy(&mut reader, &mut buf, &mut line)?;
            if n == 0 || !line.ends_with('\n') {
                break; // EOF or partial trailing line — wait for more
            }
            tail.pos += n as u64;
            if let Some(rendered) = render_if_match(&line, filter, format) {
                writeln!(out, "{rendered}")?;
                out.flush()?;
            }
        }
    }
    Ok(())
}

/// Parses one raw line, returning its rendering when it matches the filter.
/// Malformed lines (and empties) are silently skipped.
fn render_if_match(line: &str, filter: &Filter, format: Format) -> Option<String> {
    let raw = line.trim_end_matches(['\n', '\r']);
    if raw.is_empty() {
        return None;
    }
    let rec: LogRecord = serde_json::from_str(raw).ok()?;
    if filter.matches(&rec, raw) {
        Some(format::render(&rec, raw, format))
    } else {
        None
    }
}

/// Maps a broken-pipe error (downcast from `anyhow`) to a clean exit.
fn swallow_broken_pipe(e: anyhow::Error) -> Result<()> {
    if let Some(io_err) = e.downcast_ref::<io::Error>() {
        if io_err.kind() == io::ErrorKind::BrokenPipe {
            return Ok(());
        }
    }
    Err(e)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::cli::log::query::FilterInput;
    use std::io::Cursor;

    fn empty_filter() -> Filter {
        Filter::build(FilterInput {
            since: None,
            until: None,
            method: None,
            status: None,
            service: None,
            command: None,
            url: None,
            grep: None,
            fuzzy: &[],
            query: &[],
            id: None,
        })
        .unwrap()
    }

    fn sample_lines() -> String {
        let mut s = String::new();
        for i in 0..5 {
            s.push_str(&format!(
                r#"{{"id":"{i}","invocation_id":"inv","kind":"http","timestamp":"2026-06-22T00:00:0{i}.000Z","service":"gmail","method":"GET","status_code":200,"url":"/x/{i}"}}"#,
            ));
            s.push('\n');
        }
        s
    }

    #[test]
    fn backlog_emits_all_without_limit() {
        let mut reader = BufReader::new(Cursor::new(sample_lines()));
        let mut out = Vec::new();
        emit_backlog(&mut reader, &empty_filter(), Format::Json, None, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 5);
        // JSON format is byte-identical to the input lines.
        assert!(text.contains(r#""url":"/x/0""#));
        assert!(text.contains(r#""url":"/x/4""#));
    }

    #[test]
    fn backlog_limit_keeps_most_recent() {
        let mut reader = BufReader::new(Cursor::new(sample_lines()));
        let mut out = Vec::new();
        emit_backlog(
            &mut reader,
            &empty_filter(),
            Format::Json,
            Some(2),
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.contains(r#""url":"/x/3""#));
        assert!(text.contains(r#""url":"/x/4""#));
        assert!(!text.contains(r#""url":"/x/0""#));
    }

    #[test]
    fn backlog_skips_malformed_and_empty_lines() {
        let input = "not json\n\n{\"id\":\"1\",\"kind\":\"http\"}\n";
        let mut reader = BufReader::new(Cursor::new(input));
        let mut out = Vec::new();
        emit_backlog(&mut reader, &empty_filter(), Format::Json, None, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains(r#""id":"1""#));
    }

    /// A `drivemutation` line from `log.jsonl`.
    const DRIVE_MUTATION_LINE: &str = r#"{"id":"rec-1","invocation_id":"inv-3","kind":"drivemutation","timestamp":"2026-06-22T12:34:56.789Z","hostname":"host","pid":4242,"gwi_version":"0.0.1","cwd":"/work/repo","system_user":"user","command":["drive","move"],"duration_ms":17,"source":"mcp","mcp_tool":"drive_file_move","service":"drive","context":{"added_principals":"alice@example.com","crosses_drive_boundary":"true","file_id":"f1","file_name":"report.pdf","resolved_folder_id":"dest1","status":"blocked"}}"#;

    /// An `audit` line from `audit.jsonl`.
    const AUDIT_LINE: &str = r#"{"id":"rec-2","invocation_id":"inv-3","kind":"audit","timestamp":"2026-06-22T12:35:00.000Z","hostname":"host","pid":4242,"gwi_version":"0.0.1","cwd":"/work/repo","system_user":"user","command":["drive","lease-acquire"],"source":"mcp","mcp_tool":"drive_file_move","service":"drive","context":{"file_id":"f1","integration":"drive","lease_id":"lease-1","verdict":"acquired","version_after":"7"}}"#;

    #[test]
    fn backlog_renders_drive_mutation_and_audit_lines() {
        let input = format!(
            "{}\n{}\n{}\n",
            DRIVE_MUTATION_LINE,
            AUDIT_LINE,
            r#"{"id":"3","invocation_id":"inv","kind":"a-future-kind"}"#,
        );

        let mut reader = BufReader::new(Cursor::new(input.clone()));
        let mut out = Vec::new();
        emit_backlog(
            &mut reader,
            &empty_filter(),
            Format::Oneline,
            None,
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "text was: {text}");
        assert!(lines[0].contains("drv"), "line was: {}", lines[0]);
        assert!(lines[0].contains("drive move"), "line was: {}", lines[0]);
        assert!(lines[0].contains("report.pdf"), "line was: {}", lines[0]);
        assert!(
            lines[0].contains("status=blocked"),
            "line was: {}",
            lines[0]
        );
        assert!(lines[0].contains("17ms"), "line was: {}", lines[0]);
        assert!(lines[1].contains("aud"), "line was: {}", lines[1]);
        assert!(
            lines[1].contains("drive lease-acquire"),
            "line was: {}",
            lines[1]
        );
        assert!(lines[1].contains("lease=lease-1"), "line was: {}", lines[1]);
        assert!(
            lines[1].contains("verdict=acquired"),
            "line was: {}",
            lines[1]
        );

        // JSON output stays byte-identical to what is on disk.
        let mut reader = BufReader::new(Cursor::new(input));
        let mut out = Vec::new();
        emit_backlog(&mut reader, &empty_filter(), Format::Json, None, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().next(), Some(DRIVE_MUTATION_LINE));
        assert_eq!(text.lines().nth(1), Some(AUDIT_LINE));
    }

    #[test]
    fn backlog_applies_filter() {
        let filter = Filter::build(FilterInput {
            since: None,
            until: None,
            method: None,
            status: Some("5xx"),
            service: None,
            command: None,
            url: None,
            grep: None,
            fuzzy: &[],
            query: &[],
            id: None,
        })
        .unwrap();
        let mut reader = BufReader::new(Cursor::new(sample_lines()));
        let mut out = Vec::new();
        emit_backlog(&mut reader, &filter, Format::Json, None, &mut out).unwrap();
        // All sample lines are status 200, so nothing matches 5xx.
        assert!(String::from_utf8(out).unwrap().is_empty());
    }

    #[test]
    fn run_on_missing_file_without_follow_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.jsonl");
        // No file yet and not following: clean no-op.
        assert!(run(&missing, &empty_filter(), Format::Json, None, false).is_ok());
    }

    #[test]
    fn run_reads_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, sample_lines()).unwrap();
        // Exercises the open + backlog path (output goes to captured stdout).
        assert!(run(&path, &empty_filter(), Format::Json, Some(2), false).is_ok());
    }

    #[test]
    fn broken_pipe_is_swallowed_other_errors_propagate() {
        use std::io::{Error, ErrorKind};

        let mut closed = PipeWriter::failing_with(ErrorKind::BrokenPipe, 0);
        assert!(!write_line(&mut closed, "x").unwrap());
        let mut denied = PipeWriter::failing_with(ErrorKind::PermissionDenied, 0);
        assert!(write_line(&mut denied, "x").is_err());
        let mut open = Vec::new();
        assert!(write_line(&mut open, "x").unwrap());

        assert!(
            swallow_broken_pipe(anyhow::Error::from(Error::from(ErrorKind::BrokenPipe))).is_ok()
        );
        assert!(swallow_broken_pipe(anyhow::anyhow!("unrelated")).is_err());
    }

    /// A writer that accepts `ok_writes` writes, then fails every write with `kind`.
    struct PipeWriter {
        kind: io::ErrorKind,
        ok_writes: usize,
        attempts: usize,
    }

    impl PipeWriter {
        fn failing_with(kind: io::ErrorKind, ok_writes: usize) -> Self {
            Self {
                kind,
                ok_writes,
                attempts: 0,
            }
        }
    }

    impl Write for PipeWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.attempts += 1;
            if self.attempts > self.ok_writes {
                Err(io::Error::from(self.kind))
            } else {
                Ok(buf.len())
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Enough lines that the scan cannot have buffered them all in one read.
    fn many_lines() -> String {
        sample_lines().repeat(4000)
    }

    #[test]
    fn backlog_stops_at_a_closed_pipe_without_reading_the_rest() {
        let input = many_lines();
        let mut reader = BufReader::new(Cursor::new(input.clone()));
        let mut out = PipeWriter::failing_with(io::ErrorKind::BrokenPipe, 0);

        let result =
            emit_backlog(&mut reader, &empty_filter(), Format::Json, None, &mut out).unwrap();

        assert_eq!(result, Backlog::ReaderGone);
        // `writeln!` may split a record into a few writes, but never goes on to later lines.
        assert!(out.attempts <= 2, "attempts: {}", out.attempts);
        assert!(
            (reader.get_ref().position() as usize) < input.len() / 2,
            "scan read {} of {} bytes",
            reader.get_ref().position(),
            input.len()
        );
    }

    #[test]
    fn backlog_with_a_limit_stops_replaying_at_a_closed_pipe() {
        let mut reader = BufReader::new(Cursor::new(sample_lines()));
        let mut out = PipeWriter::failing_with(io::ErrorKind::BrokenPipe, 0);

        let result = emit_backlog(
            &mut reader,
            &empty_filter(),
            Format::Json,
            Some(3),
            &mut out,
        )
        .unwrap();

        assert_eq!(result, Backlog::ReaderGone);
        assert!(out.attempts <= 2, "attempts: {}", out.attempts);
    }

    #[test]
    fn backlog_propagates_other_write_errors() {
        let mut reader = BufReader::new(Cursor::new(sample_lines()));
        let mut out = PipeWriter::failing_with(io::ErrorKind::PermissionDenied, 0);
        assert!(emit_backlog(&mut reader, &empty_filter(), Format::Json, None, &mut out).is_err());
    }

    #[test]
    fn follow_loop_exits_when_the_pipe_closes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, sample_lines()).unwrap();
        let mut out = PipeWriter::failing_with(io::ErrorKind::BrokenPipe, 0);
        let tail = Tail { pos: 0, id: None };

        let err = follow_loop(&path, &empty_filter(), Format::Json, tail, &mut out).unwrap_err();

        assert!(swallow_broken_pipe(err).is_ok());
    }

    fn tail_at_start() -> Tail {
        Tail { pos: 0, id: None }
    }

    #[test]
    fn drain_appended_reads_only_new_complete_lines() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, sample_lines()).unwrap();

        // First drain reads the whole backlog and advances the position.
        let mut tail = tail_at_start();
        let mut out = Vec::new();
        drain_appended(&path, &empty_filter(), Format::Json, &mut tail, &mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap().lines().count(), 5);
        let pos = tail.pos;
        assert!(pos > 0);

        // Appending two lines and draining from `pos` yields only those.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, r#"{{"id":"5","kind":"http"}}"#).unwrap();
        writeln!(f, r#"{{"id":"6","kind":"http"}}"#).unwrap();
        let mut out = Vec::new();
        drain_appended(&path, &empty_filter(), Format::Json, &mut tail, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.contains(r#""id":"5""#));
        let pos2 = tail.pos;
        assert!(pos2 > pos);

        // A trailing partial line (no newline) is left for the next call.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        write!(f, r#"{{"id":"7","kind":"http"}}"#).unwrap();
        let mut out = Vec::new();
        drain_appended(&path, &empty_filter(), Format::Json, &mut tail, &mut out).unwrap();
        assert!(String::from_utf8(out).unwrap().is_empty());
        assert_eq!(tail.pos, pos2, "partial line does not advance the position");

        // Truncation resets to the top and re-reads.
        std::fs::write(&path, "{\"id\":\"x\",\"kind\":\"http\"}\n").unwrap();
        let mut out = Vec::new();
        drain_appended(&path, &empty_filter(), Format::Json, &mut tail, &mut out).unwrap();
        assert!(String::from_utf8(out).unwrap().contains(r#""id":"x""#));

        // A missing file is a no-op that preserves the tail.
        let missing = dir.path().join("gone.jsonl");
        let mut kept = Tail {
            pos: 7,
            id: tail.id,
        };
        let mut out = Vec::new();
        drain_appended(&missing, &empty_filter(), Format::Json, &mut kept, &mut out).unwrap();
        assert_eq!(kept.pos, 7);
        assert!(String::from_utf8(out).unwrap().is_empty());
    }

    /// Replaces `path` with `content` the way `gwi log prune` does: write a sibling,
    /// then rename it over the original.
    #[cfg(unix)]
    fn replace_by_rename(path: &Path, content: &str) {
        let sibling = path.with_extension("new");
        std::fs::write(&sibling, content).unwrap();
        std::fs::rename(&sibling, path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn drain_appended_restarts_when_the_log_is_replaced_by_a_larger_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, sample_lines()).unwrap();
        let mut tail = tail_at_start();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut Vec::new(),
        )
        .unwrap();

        // The replacement is larger than the saved offset: only the identity gives it away.
        let replacement = sample_lines().replace("/x/", "/replaced/").repeat(2);
        assert!(replacement.len() as u64 > tail.pos);
        replace_by_rename(&path, &replacement);

        let mut out = Vec::new();
        drain_appended(&path, &empty_filter(), Format::Json, &mut tail, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 10, "text was: {text}");
        assert!(text.starts_with(r#"{"id":"0""#), "text was: {text}");
        assert_eq!(tail.pos, replacement.len() as u64);
    }

    #[cfg(unix)]
    #[test]
    fn drain_appended_restarts_when_the_log_is_replaced_by_a_same_size_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, sample_lines()).unwrap();
        let mut tail = tail_at_start();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut Vec::new(),
        )
        .unwrap();

        let replacement = sample_lines().replace("/x/", "/y/");
        assert_eq!(replacement.len() as u64, tail.pos);
        replace_by_rename(&path, &replacement);

        let mut out = Vec::new();
        drain_appended(&path, &empty_filter(), Format::Json, &mut tail, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 5, "text was: {text}");
        assert!(text.contains(r#""url":"/y/0""#), "text was: {text}");
    }
}
