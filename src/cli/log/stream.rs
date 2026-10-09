//! Streaming reader: filter and render the log line by line.
//!
//! The backlog is read without buffering the whole file (a `--limit` keeps only
//! the most recent N matches in a ring buffer); `--follow` then tails newly
//! appended complete lines; a final line with no newline yet (a writer mid-append)
//! is printed by the backlog only in a one-shot run, and left for the follow loop
//! otherwise so it is printed once. A broken pipe (e.g. piping into `head`) is treated
//! as a clean exit, not an error, and ends the scan at once. A reader that goes
//! away while `--follow` is idle is noticed on the next poll tick on unix, by
//! polling stdout for hangup. On Windows and other non-unix platforms, closure is
//! noticed only on the next matching write; an idle follow may run indefinitely
//! until explicitly interrupted (accepted platform limit, #91). Backlog scans
//! also probe before reading and every 1,024 lines on unix, even when a filter or
//! `--limit` prevents writes.
//!
//! `--follow` also notices the log being replaced (`gwi log prune`, rotation):
//! by the file's identity (device and inode on unix; volume serial number and file
//! index on Windows), and on any other platform only by its shrinking. It skips
//! the contents present at detection and follows subsequent appends; replacement
//! never replays a backlog, so `--limit` applies only to the initial scan.

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

/// Amortize the hangup probe across this many backlog lines, including nonmatches.
const BACKLOG_PROBE_LINES: usize = 1024;

/// Identity of a file on disk, `(device, inode)` on unix and `(volume serial number,
/// file index)` on Windows; `None` where the platform offers none, so only
/// shrinkage reveals a replacement.
type FileId = Option<(u64, u64)>;

/// The identity of the open `file`; `None` if it cannot be read.
#[cfg(unix)]
fn file_id(file: &File) -> FileId {
    use std::os::unix::fs::MetadataExt;
    let meta = file.metadata().ok()?;
    Some((meta.dev(), meta.ino()))
}

/// The identity of the open `file`, from `GetFileInformationByHandle`; the std
/// accessors for it (`MetadataExt::file_index`, `volume_serial_number`) are unstable.
/// On NTFS the file index carries a sequence number, so a file recreated at the
/// same path does not reuse its predecessor's identity.
#[cfg(windows)]
fn file_id(file: &File) -> FileId {
    let info = winapi_util::file::information(file).ok()?;
    Some((info.volume_serial_number(), info.file_index()))
}

#[cfg(not(any(unix, windows)))]
fn file_id(_file: &File) -> FileId {
    None
}

/// Whether the reader of stdout has gone away (e.g. `| head -1` has exited), so
/// backlog scans and idle `--follow`, which may write nothing, can still stop.
#[cfg(unix)]
fn stdout_hung_up() -> bool {
    use std::os::fd::AsFd;
    fd_hung_up(io::stdout().as_fd())
}

/// Returns false off unix: a closed pipe is only found by the next write.
///
/// Accepted limit (#91): `PeekNamedPipe` requires read access, which an inherited
/// stdout write handle need not have, and may block on synchronous handles in a
/// multithreaded application. Adding a Windows API dependency alone would not
/// provide a general nonblocking probe. See the Windows API requirements:
/// <https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-peeknamedpipe>.
#[cfg(not(unix))]
fn stdout_hung_up() -> bool {
    false
}

/// Polls `fd` without waiting. A pipe whose read end is closed reports `POLLERR`
/// (Linux) or `POLLHUP` (macOS), while a tty, a file or `/dev/null` reports neither.
/// `POLLOUT` is requested because macOS reports nothing at all for an empty event
/// set. `POLLNVAL` (the fd is closed) and a failed poll are not a hangup, so this
/// can only add an exit, never a new failure.
#[cfg(unix)]
fn fd_hung_up(fd: std::os::fd::BorrowedFd<'_>) -> bool {
    use nix::poll::{poll, PollFd, PollFlags, PollTimeout};

    let mut fds = [PollFd::new(fd, PollFlags::POLLOUT)];
    match poll(&mut fds, PollTimeout::ZERO) {
        Ok(n) if n > 0 => fds[0]
            .revents()
            .is_some_and(|r| r.intersects(PollFlags::POLLERR | PollFlags::POLLHUP)),
        _ => false,
    }
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
    /// Every line was read: the offset is just past the last newline-terminated
    /// one, and `skipped` counts the newline-terminated lines that did not parse.
    Complete { pos: u64, skipped: usize },
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
            tail.id = file_id(&file);
            let mut reader = BufReader::new(file);
            match emit_backlog(
                &mut reader,
                filter,
                format,
                limit,
                follow,
                &mut out,
                stdout_hung_up,
            )? {
                Backlog::Complete { pos, skipped } => {
                    tail.pos = pos;
                    warn_skipped(path, skipped);
                }
                Backlog::ReaderGone => return Ok(()),
            }
            warn_unmatched_terms(filter, follow);
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if !follow {
                return Ok(());
            }
        }
        Err(e) => return Err(e.into()),
    }

    if follow {
        if let Err(e) = follow_loop(path, filter, format, tail, &mut out, stdout_hung_up) {
            return swallow_broken_pipe(e);
        }
    }
    Ok(())
}

/// The warning for `skipped` lines of `path` that could not be parsed, or `None`
/// when there were none.
fn skipped_warning(path: &Path, skipped: usize) -> Option<String> {
    let lines = match skipped {
        0 => return None,
        1 => "1 unparseable line".to_string(),
        n => format!("{n} unparseable lines"),
    };
    Some(format!("warning: skipped {lines} in {}", path.display()))
}

/// Prints the skipped-lines warning to stderr, once the backlog has been scanned:
/// a corrupt record is dropped from every search, so a short or empty result
/// would otherwise read as "nothing happened". Stderr only, so stdout and the
/// exit code are unchanged. Under `--follow` only the backlog is counted.
fn warn_skipped(path: &Path, skipped: usize) {
    if let Some(warning) = skipped_warning(path, skipped) {
        // Best effort: a closed stderr must not fail the search.
        let _ = writeln!(io::stderr().lock(), "{warning}");
    }
}

/// Prints the filter's unmatched-term warnings to stderr, once the backlog has
/// been scanned: a query field that is no built-in name and appears in no
/// record's `context`, or a status word that no `drivemutation` record has,
/// would otherwise just match nothing. Stderr only, so the output stays
/// machine-readable and the exit code is unchanged. While following, a warning
/// is not revised if the field or status turns up later, so it says "so far".
fn warn_unmatched_terms(filter: &Filter, following: bool) {
    let mut err = io::stderr().lock();
    let warnings = filter
        .unknown_field_warnings(following)
        .into_iter()
        .chain(filter.unseen_status_warnings(following));
    for warning in warnings {
        // Best effort: a closed stderr must not fail the search.
        let _ = writeln!(err, "{warning}");
    }
}

/// Reads the next line (through its `\n`, if any) into `line`, returning the byte
/// count. Invalid UTF-8 is replaced rather than failing the read, so one corrupt
/// line cannot abort the scan; such a line is then reported by [`parse_line`] as
/// [`Line::Malformed`] if it no longer parses, as any other malformed line is.
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
/// newline-terminated line, and how many of those lines did not parse. A trailing
/// partial line (a writer mid-append) is neither counted as read nor as malformed,
/// so when `follow` is set it is also not printed: `--follow` reads it from that
/// offset and prints it once, when it is complete. A one-shot scan has nothing to
/// complete it, so it still prints the line if it parses.
/// Stops at the first write to a closed pipe or when `reader_gone` reports hangup,
/// checked before reading and every [`BACKLOG_PROBE_LINES`] lines thereafter.
/// The cadence counts every line, regardless of parsing or filtering.
fn emit_backlog<R: BufRead, W: Write>(
    reader: &mut R,
    filter: &Filter,
    format: Format,
    limit: Option<usize>,
    follow: bool,
    out: &mut W,
    mut reader_gone: impl FnMut() -> bool,
) -> Result<Backlog> {
    let mut pos = 0u64;
    let mut skipped = 0usize;
    let mut ring: VecDeque<String> = VecDeque::new();
    let mut buf = Vec::new();
    let mut line = String::new();
    let mut until_probe = 0;
    loop {
        if until_probe == 0 {
            if reader_gone() {
                return Ok(Backlog::ReaderGone);
            }
            until_probe = BACKLOG_PROBE_LINES;
        }
        until_probe -= 1;
        let n = read_line_lossy(reader, &mut buf, &mut line)?;
        if n == 0 {
            break;
        }
        let complete = line.ends_with('\n');
        if complete {
            pos += n as u64;
        } else if follow {
            break; // a partial last line: left for the follow loop to print once
        }
        match parse_line(&line, filter, format) {
            Line::Blank | Line::Filtered => {}
            Line::Malformed => skipped += usize::from(complete),
            Line::Match(rendered) => match limit {
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
            },
        }
    }
    for rendered in &ring {
        if !write_line(out, rendered)? {
            return Ok(Backlog::ReaderGone);
        }
    }
    Ok(Backlog::Complete { pos, skipped })
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
/// (until the process is interrupted, a write finds the pipe closed, or
/// `reader_gone` reports that the reader has gone away, which is checked each
/// tick so an idle follow, which never writes, still stops). Resumes from the
/// end when the file is truncated or replaced, without replaying its contents.
fn follow_loop<W: Write>(
    path: &Path,
    filter: &Filter,
    format: Format,
    mut tail: Tail,
    out: &mut W,
    mut reader_gone: impl FnMut() -> bool,
) -> Result<()> {
    loop {
        // Before the drain, so a reader that left during the sleep ends the loop
        // cleanly even if the drain would fail.
        if reader_gone() {
            return Ok(());
        }
        drain_appended(path, filter, format, &mut tail, out)?;
        std::thread::sleep(FOLLOW_POLL);
    }
}

/// Reads and emits any complete lines appended past `tail.pos`, advancing the
/// tail. Skips to the observed end if the file was replaced (its identity changed)
/// or shrank (truncation); a no-op if the file is absent or has not grown.
/// This also applies to replacement during the initial backlog scan.
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
    let len = file.metadata().map_or(tail.pos, |m| m.len());
    let id = file_id(&file);
    // An identity that cannot be read says nothing about replacement: keep the saved
    // one, and do not take a first successful read after a failure for a change.
    let replaced = matches!((id, tail.id), (Some(now), Some(saved)) if now != saved);
    if replaced || len < tail.pos {
        // Prune retains records already seen, and identity cannot tell it apart
        // from rotation. Never replay replacement contents, including records
        // written before this poll; follow only subsequent appends.
        tail.pos = len;
    }
    tail.id = id.or(tail.id);
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
            // A corrupt line appended while following is not reported: only the
            // backlog is counted.
            if let Line::Match(rendered) = parse_line(&line, filter, format) {
                writeln!(out, "{rendered}")?;
                out.flush()?;
            }
        }
    }
    Ok(())
}

/// What one raw log line turned out to be.
#[derive(Debug, PartialEq, Eq)]
enum Line {
    /// Empty or only whitespace: not a damaged record, so not counted.
    Blank,
    /// Not a log record.
    Malformed,
    /// A record the filter rejected.
    Filtered,
    /// A record the filter accepted, rendered.
    Match(String),
}

/// Parses one raw line and renders it when it matches the filter.
fn parse_line(line: &str, filter: &Filter, format: Format) -> Line {
    let raw = line.trim_end_matches(['\n', '\r']);
    if raw.trim().is_empty() {
        return Line::Blank;
    }
    let Ok(rec) = serde_json::from_str::<LogRecord>(raw) else {
        return Line::Malformed;
    };
    if filter.matches(&rec, raw) {
        Line::Match(format::render(&rec, raw, format))
    } else {
        Line::Filtered
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

    fn filter_input() -> FilterInput<'static> {
        FilterInput {
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
        }
    }

    fn empty_filter() -> Filter {
        Filter::build(filter_input()).unwrap()
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
        emit_backlog(
            &mut reader,
            &empty_filter(),
            Format::Json,
            None,
            false,
            &mut out,
            || false,
        )
        .unwrap();
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
            false,
            &mut out,
            || false,
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
        emit_backlog(
            &mut reader,
            &empty_filter(),
            Format::Json,
            None,
            false,
            &mut out,
            || false,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains(r#""id":"1""#));
    }

    /// A complete record with no trailing newline: a writer caught mid-append.
    const UNTERMINATED: &str = r#"{"id":"tail","kind":"http","url":"/tail"}"#;

    #[test]
    fn one_shot_backlog_prints_an_unterminated_final_line() {
        let complete = sample_lines();
        let input = format!("{complete}{UNTERMINATED}");
        let mut reader = BufReader::new(Cursor::new(input));
        let mut out = Vec::new();

        let result = emit_backlog(
            &mut reader,
            &empty_filter(),
            Format::Json,
            None,
            false,
            &mut out,
            || false,
        )
        .unwrap();

        // Nothing will complete the line, so it is printed; the offset still stops before it.
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 6, "text was: {text}");
        assert_eq!(text.lines().last(), Some(UNTERMINATED));
        assert_eq!(
            result,
            Backlog::Complete {
                pos: complete.len() as u64,
                skipped: 0
            }
        );
    }

    #[test]
    fn following_backlog_holds_back_an_unterminated_final_line() {
        let complete = sample_lines();
        let input = format!("{complete}{UNTERMINATED}");
        let mut reader = BufReader::new(Cursor::new(input));
        let mut out = Vec::new();

        let result = emit_backlog(
            &mut reader,
            &empty_filter(),
            Format::Json,
            None,
            true,
            &mut out,
            || false,
        )
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 5, "text was: {text}");
        assert!(!text.contains("/tail"), "text was: {text}");
        assert_eq!(
            result,
            Backlog::Complete {
                pos: complete.len() as u64,
                skipped: 0
            }
        );
    }

    #[test]
    fn following_backlog_with_a_limit_does_not_spend_a_slot_on_the_held_back_line() {
        let input = format!("{}{UNTERMINATED}", sample_lines());
        let mut reader = BufReader::new(Cursor::new(input));
        let mut out = Vec::new();

        emit_backlog(
            &mut reader,
            &empty_filter(),
            Format::Json,
            Some(2),
            true,
            &mut out,
            || false,
        )
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 2, "text was: {text}");
        assert!(text.contains(r#""url":"/x/3""#), "text was: {text}");
        assert!(text.contains(r#""url":"/x/4""#), "text was: {text}");
    }

    #[test]
    fn following_backlog_prints_a_terminated_final_line() {
        let input = format!("{}{UNTERMINATED}\n", sample_lines());
        let mut reader = BufReader::new(Cursor::new(input.clone()));
        let mut out = Vec::new();

        let result = emit_backlog(
            &mut reader,
            &empty_filter(),
            Format::Json,
            None,
            true,
            &mut out,
            || false,
        )
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 6, "text was: {text}");
        assert_eq!(
            result,
            Backlog::Complete {
                pos: input.len() as u64,
                skipped: 0
            }
        );
    }

    #[test]
    fn follow_prints_a_record_once_when_it_starts_mid_append() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, format!("{}{UNTERMINATED}", sample_lines())).unwrap();

        // The backlog scan, as `run` does it with `--follow`.
        let file = File::open(&path).unwrap();
        let id = file_id(&file);
        let mut reader = BufReader::new(file);
        let mut backlog = Vec::new();
        let Backlog::Complete { pos, .. } = emit_backlog(
            &mut reader,
            &empty_filter(),
            Format::Json,
            None,
            true,
            &mut backlog,
            || false,
        )
        .unwrap() else {
            panic!("the scan should complete");
        };
        let mut tail = Tail { pos, id };

        // The writer has not finished: the follow loop prints nothing yet.
        let mut out = Vec::new();
        drain_appended(&path, &empty_filter(), Format::Json, &mut tail, &mut out).unwrap();
        assert!(out.is_empty());

        // The writer finishes the line.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f).unwrap();
        drain_appended(&path, &empty_filter(), Format::Json, &mut tail, &mut out).unwrap();

        let all = format!(
            "{}{}",
            String::from_utf8(backlog).unwrap(),
            String::from_utf8(out).unwrap()
        );
        assert_eq!(all.matches("/tail").count(), 1, "output was: {all}");
        assert_eq!(all.lines().count(), 6, "output was: {all}");
    }

    /// Runs the backlog scan over `input`, returning its stdout and the result.
    fn scan(input: &[u8], filter: &Filter, limit: Option<usize>) -> (String, Backlog) {
        let mut reader = BufReader::new(Cursor::new(input.to_vec()));
        let mut out = Vec::new();
        let result = emit_backlog(
            &mut reader,
            filter,
            Format::Json,
            limit,
            false,
            &mut out,
            || false,
        )
        .unwrap();
        (String::from_utf8(out).unwrap(), result)
    }

    const GOOD: &str = r#"{"id":"1","kind":"http"}"#;

    #[test]
    fn backlog_counts_unparseable_lines_and_leaves_stdout_alone() {
        let mut input = format!("not json\n{GOOD}\n{{\"id\":\n").into_bytes();
        input.extend_from_slice(b"\xff\xfe\n"); // invalid UTF-8
        input.extend_from_slice(format!("{GOOD}\n").as_bytes());
        let (text, result) = scan(&input, &empty_filter(), None);
        assert_eq!(text, format!("{GOOD}\n{GOOD}\n"));
        assert_eq!(
            result,
            Backlog::Complete {
                pos: input.len() as u64,
                skipped: 3
            }
        );
    }

    #[test]
    fn backlog_does_not_count_clean_blank_or_partial_lines() {
        let clean = format!("{GOOD}\n{GOOD}\n");
        let (_, result) = scan(clean.as_bytes(), &empty_filter(), None);
        assert_eq!(
            result,
            Backlog::Complete {
                pos: clean.len() as u64,
                skipped: 0
            }
        );

        let blanks = format!("\n{GOOD}\n\r\n  \t\n\n");
        let (_, result) = scan(blanks.as_bytes(), &empty_filter(), None);
        assert_eq!(
            result,
            Backlog::Complete {
                pos: blanks.len() as u64,
                skipped: 0
            }
        );

        // A writer mid-append: neither read nor malformed yet.
        let partial = format!("{GOOD}\n{{\"id\":\"2\",\"ki");
        let (text, result) = scan(partial.as_bytes(), &empty_filter(), None);
        assert_eq!(text, format!("{GOOD}\n"));
        assert_eq!(
            result,
            Backlog::Complete {
                pos: GOOD.len() as u64 + 1,
                skipped: 0
            }
        );
    }

    #[test]
    fn backlog_counts_unparseable_lines_the_filter_and_limit_would_hide() {
        let input = format!("junk\n{GOOD}\njunk\n{GOOD}\njunk\n");
        let skipped = |result| match result {
            Backlog::Complete { skipped, .. } => skipped,
            Backlog::ReaderGone => panic!("reader gone"),
        };
        let (text, result) = scan(input.as_bytes(), &empty_filter(), Some(1));
        assert_eq!(text.lines().count(), 1);
        assert_eq!(skipped(result), 3);

        let filter = Filter::build(FilterInput {
            status: Some("5xx"),
            ..filter_input()
        })
        .unwrap();
        let (text, result) = scan(input.as_bytes(), &filter, None);
        assert!(text.is_empty());
        assert_eq!(skipped(result), 3);
    }

    #[test]
    fn skipped_warning_names_the_count_and_the_path() {
        let path = Path::new("/state/log.jsonl");
        assert_eq!(skipped_warning(path, 0), None);
        assert_eq!(
            skipped_warning(path, 1).as_deref(),
            Some("warning: skipped 1 unparseable line in /state/log.jsonl")
        );
        assert_eq!(
            skipped_warning(path, 3).as_deref(),
            Some("warning: skipped 3 unparseable lines in /state/log.jsonl")
        );
    }

    #[test]
    fn drain_appended_ignores_corrupt_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, format!("junk\n{GOOD}\n")).unwrap();
        let mut tail = tail_at_start();
        let mut out = Vec::new();
        drain_appended(&path, &empty_filter(), Format::Json, &mut tail, &mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), format!("{GOOD}\n"));
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
            false,
            &mut out,
            || false,
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
        emit_backlog(
            &mut reader,
            &empty_filter(),
            Format::Json,
            None,
            false,
            &mut out,
            || false,
        )
        .unwrap();
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
        emit_backlog(
            &mut reader,
            &filter,
            Format::Json,
            None,
            false,
            &mut out,
            || false,
        )
        .unwrap();
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
    fn backlog_probe_stops_scans_that_write_nothing() {
        let input = many_lines();
        let mut no_match = filter_input();
        let queries = ["rare:1".to_string()];
        no_match.query = &queries;
        let no_match = Filter::build(no_match).unwrap();
        for (filter, limit) in [(&no_match, None), (&empty_filter(), Some(3))] {
            for follow in [false, true] {
                let mut reader = Cursor::new(input.as_bytes());
                let mut out = Vec::new();
                let mut probes = 0;
                let result = emit_backlog(
                    &mut reader,
                    filter,
                    Format::Json,
                    limit,
                    follow,
                    &mut out,
                    || {
                        probes += 1;
                        probes == 2
                    },
                )
                .unwrap();
                assert_eq!(result, Backlog::ReaderGone);
                assert_eq!(probes, 2);
                assert!(out.is_empty());
                let consumed = reader.position() as usize;
                assert!(consumed < input.len());
                assert_eq!(input[..consumed].lines().count(), BACKLOG_PROBE_LINES);
            }
        }
    }

    #[test]
    fn backlog_probe_counts_blank_and_malformed_lines() {
        for line in ["\n", "not json\n"] {
            let input = line.repeat(BACKLOG_PROBE_LINES * 3);
            let mut reader = Cursor::new(input.as_bytes());
            let mut out = Vec::new();
            let mut probes = 0;
            let result = emit_backlog(
                &mut reader,
                &empty_filter(),
                Format::Json,
                None,
                false,
                &mut out,
                || {
                    probes += 1;
                    probes == 2
                },
            )
            .unwrap();
            assert_eq!(result, Backlog::ReaderGone);
            assert_eq!(reader.position() as usize, line.len() * BACKLOG_PROBE_LINES);
            assert_eq!(probes, 2);
            assert!(out.is_empty());
        }
    }

    #[test]
    fn backlog_probe_detects_an_already_closed_reader_before_reading() {
        let mut reader = Cursor::new(sample_lines());
        let mut out = Vec::new();
        let result = emit_backlog(
            &mut reader,
            &empty_filter(),
            Format::Json,
            None,
            false,
            &mut out,
            || true,
        )
        .unwrap();
        assert_eq!(result, Backlog::ReaderGone);
        assert_eq!(reader.position(), 0);
        assert!(out.is_empty());
    }

    #[test]
    fn backlog_probe_is_amortized_and_preserves_live_reader_output() {
        let input = many_lines();
        let mut reader = Cursor::new(input.as_bytes());
        let mut out = Vec::new();
        let mut probes = 0;
        let result = emit_backlog(
            &mut reader,
            &empty_filter(),
            Format::Json,
            None,
            false,
            &mut out,
            || {
                probes += 1;
                false
            },
        )
        .unwrap();
        assert_eq!(
            result,
            Backlog::Complete {
                pos: input.len() as u64,
                skipped: 0
            }
        );
        assert_eq!(out, input.as_bytes());
        assert_eq!(probes, input.lines().count() / BACKLOG_PROBE_LINES + 1);
    }

    #[test]
    fn backlog_stops_at_a_closed_pipe_without_reading_the_rest() {
        let input = many_lines();
        let mut reader = BufReader::new(Cursor::new(input.clone()));
        let mut out = PipeWriter::failing_with(io::ErrorKind::BrokenPipe, 0);

        let result = emit_backlog(
            &mut reader,
            &empty_filter(),
            Format::Json,
            None,
            false,
            &mut out,
            || false,
        )
        .unwrap();

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
            false,
            &mut out,
            || false,
        )
        .unwrap();

        assert_eq!(result, Backlog::ReaderGone);
        assert!(out.attempts <= 2, "attempts: {}", out.attempts);
    }

    #[test]
    fn backlog_propagates_other_write_errors() {
        let mut reader = BufReader::new(Cursor::new(sample_lines()));
        let mut out = PipeWriter::failing_with(io::ErrorKind::PermissionDenied, 0);
        assert!(emit_backlog(
            &mut reader,
            &empty_filter(),
            Format::Json,
            None,
            false,
            &mut out,
            || false
        )
        .is_err());
    }

    #[test]
    fn follow_loop_exits_when_the_pipe_closes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, sample_lines()).unwrap();
        let mut out = PipeWriter::failing_with(io::ErrorKind::BrokenPipe, 0);
        let tail = Tail { pos: 0, id: None };

        let err = follow_loop(&path, &empty_filter(), Format::Json, tail, &mut out, || {
            false
        })
        .unwrap_err();

        assert!(swallow_broken_pipe(err).is_ok());
    }

    #[test]
    fn follow_loop_exits_when_the_reader_goes_away_while_idle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, "").unwrap();
        let mut out = Vec::new();
        let mut ticks = 0;

        // Nothing is ever written, so only the hangup check can end the loop.
        follow_loop(
            &path,
            &empty_filter(),
            Format::Json,
            tail_at_start(),
            &mut out,
            || {
                ticks += 1;
                ticks == 3
            },
        )
        .unwrap();

        assert_eq!(ticks, 3);
        assert!(out.is_empty());
    }

    #[test]
    fn follow_loop_prefers_the_hangup_exit_to_a_failing_drain() {
        let dir = tempfile::tempdir().unwrap();
        // A directory where the log should be: opening it succeeds but reading fails.
        let path = dir.path().join("log.jsonl");
        std::fs::create_dir(&path).unwrap();
        let tail = Tail { pos: 0, id: None };
        let mut out = Vec::new();

        let result = follow_loop(&path, &empty_filter(), Format::Json, tail, &mut out, || {
            true
        });

        assert!(result.is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn fd_hung_up_is_true_only_once_the_read_end_of_a_pipe_closes() {
        use std::os::fd::AsFd;

        let (read, write) = nix::unistd::pipe().unwrap();
        assert!(!fd_hung_up(write.as_fd()));
        drop(read);
        assert!(fd_hung_up(write.as_fd()));
    }

    #[cfg(unix)]
    #[test]
    fn fd_hung_up_is_false_for_files_and_a_writable_pipe_with_its_reader() {
        use std::os::fd::AsFd;

        let dir = tempfile::tempdir().unwrap();
        let file = File::create(dir.path().join("out")).unwrap();
        assert!(!fd_hung_up(file.as_fd()));
        let null = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/null")
            .unwrap();
        assert!(!fd_hung_up(null.as_fd()));
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

        // Truncation skips existing contents and follows subsequent appends.
        std::fs::write(&path, "{\"id\":\"x\",\"kind\":\"http\"}\n").unwrap();
        let mut out = Vec::new();
        drain_appended(&path, &empty_filter(), Format::Json, &mut tail, &mut out).unwrap();
        assert!(out.is_empty());
        assert_eq!(tail.pos, std::fs::metadata(&path).unwrap().len());
        assert_follows_next_append(&path, &mut tail);

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

    #[cfg(any(unix, windows))]
    #[test]
    fn file_id_is_stable_for_one_file_and_differs_between_files() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.jsonl"), dir.path().join("b.jsonl"));
        std::fs::write(&a, sample_lines()).unwrap();
        std::fs::write(&b, sample_lines()).unwrap();
        let id_of = |path: &Path| file_id(&File::open(path).unwrap());

        let first = id_of(&a);
        assert!(first.is_some());
        // Growing the file in place does not change its identity.
        let mut f = std::fs::OpenOptions::new().append(true).open(&a).unwrap();
        writeln!(f, "{{}}").unwrap();
        assert_eq!(id_of(&a), first);
        assert_ne!(id_of(&b), first);
    }

    #[test]
    fn drain_appended_adopts_an_identity_learned_late_without_replaying() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, sample_lines()).unwrap();
        // The identity could not be read at startup (`None`), and the backlog is behind us.
        let mut tail = Tail {
            pos: sample_lines().len() as u64,
            id: None,
        };
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, r#"{{"id":"5","kind":"http"}}"#).unwrap();

        let mut out = Vec::new();
        drain_appended(&path, &empty_filter(), Format::Json, &mut tail, &mut out).unwrap();

        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 1, "text was: {text}");
        assert!(text.contains(r#""id":"5""#));
        assert_eq!(tail.id.is_some(), cfg!(any(unix, windows)));
    }

    /// Replaces `path` with `content` the way `gwi log prune` does: write a sibling,
    /// then rename it over the original.
    #[cfg(any(unix, windows))]
    fn replace_by_rename(path: &Path, content: &str) {
        let sibling = path.with_extension("new");
        std::fs::write(&sibling, content).unwrap();
        std::fs::rename(&sibling, path).unwrap();
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn drain_appended_skips_contents_when_the_log_is_replaced_by_a_larger_file() {
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
        assert!(text.is_empty(), "text was: {text}");
        assert_eq!(tail.pos, replacement.len() as u64);
        assert_follows_next_append(&path, &mut tail);
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn drain_appended_skips_contents_when_the_log_is_replaced_by_a_same_size_file() {
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
        assert!(text.is_empty(), "text was: {text}");
        assert_eq!(tail.pos, replacement.len() as u64);
        assert_follows_next_append(&path, &mut tail);
    }

    fn assert_follows_next_append(path: &Path, tail: &mut Tail) {
        let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        writeln!(file, "{GOOD}").unwrap();
        let mut out = Vec::new();
        drain_appended(path, &empty_filter(), Format::Json, tail, &mut out).unwrap();
        assert_eq!(out, format!("{GOOD}\n").as_bytes());
        out.clear();
        drain_appended(path, &empty_filter(), Format::Json, tail, &mut out).unwrap();
        assert!(out.is_empty(), "the next append is printed exactly once");
    }

    #[test]
    fn drain_appended_reads_a_file_created_after_a_missing_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        let mut tail = tail_at_start();
        let mut out = Vec::new();
        drain_appended(&path, &empty_filter(), Format::Json, &mut tail, &mut out).unwrap();
        std::fs::write(&path, sample_lines()).unwrap();
        drain_appended(&path, &empty_filter(), Format::Json, &mut tail, &mut out).unwrap();
        assert_eq!(out, sample_lines().as_bytes());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn replacement_during_or_after_backlog_never_replays_even_with_a_limit() {
        for during_scan in [false, true] {
            for limit in [None, Some(0), Some(2)] {
                for rotation in [false, true] {
                    let dir = tempfile::tempdir().unwrap();
                    let path = dir.path().join("log.jsonl");
                    let original = sample_lines();
                    std::fs::write(&path, &original).unwrap();
                    let file = File::open(&path).unwrap();
                    let mut tail = Tail {
                        pos: 0,
                        id: file_id(&file),
                    };
                    let mut reader = BufReader::new(file);
                    // Prune retains the last two old records; rotation contains all-new IDs.
                    let replacement = if rotation {
                        original.replace("\"id\":\"", "\"id\":\"new-")
                    } else {
                        original.split_inclusive('\n').skip(3).collect()
                    };
                    let mut out = Vec::new();
                    let result = emit_backlog(
                        &mut reader,
                        &empty_filter(),
                        Format::Json,
                        limit,
                        true,
                        &mut out,
                        || {
                            if during_scan {
                                replace_by_rename(&path, &replacement);
                            }
                            false
                        },
                    )
                    .unwrap();
                    let Backlog::Complete { pos, .. } = result else {
                        panic!("backlog should complete");
                    };
                    tail.pos = pos;
                    assert_eq!(
                        out.split(|byte| *byte == b'\n')
                            .filter(|line| !line.is_empty())
                            .count(),
                        limit.unwrap_or(5)
                    );
                    if !during_scan {
                        replace_by_rename(&path, &replacement);
                    }
                    out.clear();
                    drain_appended(&path, &empty_filter(), Format::Json, &mut tail, &mut out)
                        .unwrap();
                    assert!(out.is_empty(), "replacement must never replay records");
                    assert_eq!(tail.pos, replacement.len() as u64);
                    assert_follows_next_append(&path, &mut tail);
                }
            }
        }
    }
}
