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
//! and appended drains probe before reading and every 1,024 lines on unix, even
//! when a filter or `--limit` prevents writes. Backlog line acquisition also
//! probes every 256 KiB consumed, before decoding or parsing an oversized line.
//! Buffered prefetch, blocking I/O and parsing a completed line are outside
//! this byte bound.
//!
//! `--follow` also notices the log being replaced (`gwi log prune`, rotation):
//! by the file's identity (device and inode on unix; volume serial number and file
//! index on Windows), and on all platforms by shrinkage or changed checkpoint
//! bytes. On unix,
//! the previous file stays open between polls so its inode cannot be reused by
//! multiple replacements. It skips the contents present at detection and follows subsequent appends; replacement
//! never replays a backlog, so `--limit` applies only to the initial scan.
//! A checkpoint checks at most 256 bytes before the saved offset per poll.
//! Rewrites preserving those bytes, or racing non-atomic reads, can escape detection.

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

use super::format;
use super::query::Filter;
use super::Format;
use crate::request_log::LogRecord;

/// Poll interval while following the log.
const FOLLOW_POLL: Duration = Duration::from_millis(250);

/// Maximum raw bytes checked immediately before the saved follow offset.
const TAIL_CHECKPOINT_BYTES: usize = 256;

/// Amortizes the hangup probe across this many scanned lines, including nonmatches.
const HANGUP_PROBE_LINES: usize = 1024;

/// Maximum physical line numbers included in a skipped-lines warning.
const SKIPPED_POSITION_CAP: usize = 5;

/// Total malformed lines and a bounded sample of their physical line numbers.
#[derive(Debug, Default, PartialEq, Eq)]
struct SkippedLines {
    count: usize,
    positions: Vec<u64>,
}

impl SkippedLines {
    /// Records a malformed line without retaining more than the warning cap.
    fn record(&mut self, line: u64) {
        self.count += 1;
        if self.positions.len() < SKIPPED_POSITION_CAP {
            self.positions.push(line);
        }
    }
}

/// Maximum bytes consumed into line storage between acquisition probes.
/// This is a probe budget, not a record-size limit; full records remain intact.
const LINE_PROBE_BYTES: usize = 256 * 1024;

/// Identity of a file on disk, `(device, inode)` on unix and `(volume serial number,
/// file index)` on Windows; `None` where the platform offers none, so only
/// shrinkage or changed checkpoint bytes reveal a replacement.
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
#[derive(Debug, Default)]
struct Tail {
    /// Whether an open failure has been reported since the last successful open.
    open_failure_reported: bool,
    /// Byte offset just past the last complete line read.
    pos: u64,
    /// Number of newlines consumed before `pos` in this file.
    lines: u64,
    /// Raw bytes immediately before `pos`, bounded by `TAIL_CHECKPOINT_BYTES`.
    checkpoint: Vec<u8>,
    /// Identity of the file `pos` refers to, when known.
    id: FileId,
    /// Pins the Unix inode until a fresh file has been opened and read, preventing
    /// two replacements between polls from reusing the saved identity.
    #[cfg(unix)]
    file: Option<File>,
}

/// How the backlog scan ended.
#[derive(Debug, PartialEq, Eq)]
enum Backlog {
    /// Every line was read: the offset is just past the last newline-terminated
    /// one, `lines` counts newlines consumed, and `skipped` summarizes malformed lines.
    Complete {
        pos: u64,
        lines: u64,
        skipped: SkippedLines,
    },
    /// The output pipe closed (e.g. `| head -1`), so the scan stopped early.
    ReaderGone,
}

/// Returns numbered backups oldest first, followed by the live file.
/// Discovers files without assuming contiguous suffixes or current retention settings.
fn backlog_paths(path: &Path, rotated: bool) -> Result<Vec<PathBuf>> {
    let mut backups = Vec::new();
    if rotated {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let entries = match std::fs::read_dir(parent) {
            Ok(entries) => Some(entries),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("Failed to read log directory {}", parent.display()))
            }
        };
        if let (Some(entries), Some(name)) = (entries, path.file_name()) {
            // Strip the exact OsStr prefix first, preserving non-UTF-8 log filenames.
            let mut prefix = name.to_os_string();
            prefix.push(".");
            for entry in entries {
                let entry = entry?;
                let filename = entry.file_name();
                let Some(generation) = backup_generation(&filename, &prefix) else {
                    continue;
                };
                if entry.file_type()?.is_file() {
                    backups.push((generation, entry.path()));
                }
            }
        }
    }
    backups.sort_unstable_by_key(|(generation, _)| std::cmp::Reverse(*generation));
    let mut paths: Vec<_> = backups.into_iter().map(|(_, path)| path).collect();
    paths.push(path.to_path_buf());
    Ok(paths)
}

/// Returns the canonical positive generation suffix after the exact log-name prefix.
fn backup_generation(filename: &OsStr, prefix: &OsStr) -> Option<u32> {
    let suffix = filename
        .as_encoded_bytes()
        .strip_prefix(prefix.as_encoded_bytes())?;
    let suffix = std::str::from_utf8(suffix).ok()?;
    let generation = suffix.parse::<u32>().ok()?;
    (generation > 0 && suffix == generation.to_string()).then_some(generation)
}

/// Streams the selected backlog, then optionally follows only the live file.
pub fn run(
    path: &Path,
    filter: &Filter,
    format: Format,
    limit: Option<usize>,
    follow: bool,
    rotated: bool,
) -> Result<()> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut sink = BacklogSink {
        format,
        limit,
        ring: VecDeque::new(),
        out: &mut out,
    };
    let Some(tail) = scan_files(path, filter, follow, rotated, &mut sink, stdout_hung_up)? else {
        return Ok(());
    };
    if follow {
        if let Err(e) = follow_loop(path, filter, format, tail, &mut out, stdout_hung_up) {
            return swallow_broken_pipe(e);
        }
    }
    Ok(())
}

/// Scans the selected files with shared output state and returns the live-file tail.
/// Returns `None` if the output reader has gone away.
fn scan_files<W: Write>(
    path: &Path,
    filter: &Filter,
    follow: bool,
    rotated: bool,
    sink: &mut BacklogSink<'_, W>,
    mut reader_gone: impl FnMut() -> bool,
) -> Result<Option<Tail>> {
    let mut tail = Tail::default();
    let mut scanned = false;
    for current in backlog_paths(path, rotated)? {
        let file = match File::open(&current) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("Failed to open log file {}", current.display()))
            }
        };
        let live = current == path;
        if live {
            tail.id = file_id(&file);
        }
        let mut reader = BufReader::new(file);
        match scan_backlog(&mut reader, filter, follow && live, sink, &mut reader_gone)? {
            Backlog::Complete {
                pos,
                lines,
                skipped,
            } => {
                if live {
                    tail.pos = pos;
                    tail.lines = lines;
                    if follow {
                        tail.checkpoint = read_checkpoint(reader.get_ref(), pos)?;
                    }
                }
                warn_skipped(&current, &skipped);
            }
            Backlog::ReaderGone => return Ok(None),
        }
        #[cfg(unix)]
        if live && follow {
            tail.file = Some(reader.into_inner());
        }
        scanned = true;
    }
    if !sink.finish()? {
        return Ok(None);
    }
    if scanned {
        warn_unmatched_terms(filter, follow);
    }
    Ok(Some(tail))
}

/// Rendering and the single limit buffer shared by every backlog file.
struct BacklogSink<'a, W> {
    format: Format,
    limit: Option<usize>,
    ring: VecDeque<String>,
    out: &'a mut W,
}

impl<W: Write> BacklogSink<'_, W> {
    /// Streams one match or retains it among the most recent matches.
    fn emit(&mut self, rendered: String) -> io::Result<bool> {
        match self.limit {
            Some(cap) => {
                if cap > 0 {
                    if self.ring.len() == cap {
                        self.ring.pop_front();
                    }
                    self.ring.push_back(rendered);
                }
                Ok(true)
            }
            None => write_line(self.out, &rendered),
        }
    }

    /// Emits the retained matches after all files have been scanned.
    fn finish(&mut self) -> io::Result<bool> {
        for rendered in &self.ring {
            if !write_line(self.out, rendered)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// The warning for `skipped` lines of `path` that could not be parsed, or `None`
/// when there were none.
fn skipped_warning(path: &Path, skipped: &SkippedLines) -> Option<String> {
    let lines = match skipped.count {
        0 => return None,
        1 => "1 unparseable line".to_string(),
        n => format!("{n} unparseable lines"),
    };
    let positions = skipped
        .positions
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let more = skipped.count - skipped.positions.len();
    let suffix = if more == 0 {
        String::new()
    } else {
        format!(" and {more} more")
    };
    Some(format!(
        "warning: skipped {lines} in {} (lines {positions}{suffix})",
        path.display()
    ))
}

/// Prints the skipped-lines warning to stderr, once the backlog has been scanned:
/// a corrupt record is dropped from every search, so a short or empty result
/// would otherwise read as "nothing happened". Stderr only, so stdout and the
/// exit code are unchanged. The follow loop reports the count of newly consumed malformed
/// lines and their positions once per drain pass using the same warning.
fn warn_skipped(path: &Path, skipped: &SkippedLines) {
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
/// waits for relevant evidence, then is delivered once and says "so far".
fn warn_unmatched_terms(filter: &Filter, following: bool) {
    let mut err = io::stderr().lock();
    let warnings = if following {
        filter.pending_follow_warnings()
    } else {
        filter
            .unknown_field_warnings(false)
            .into_iter()
            .chain(filter.unseen_status_warnings(false))
            .collect()
    };
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
    // The follow drain retains its line-based probe cadence. Share acquisition
    // semantics while leaving its existing EOF/partial-line behavior intact.
    let mut until_probe = LINE_PROBE_BYTES;
    read_line_lossy_interruptible(reader, buf, line, &mut until_probe, &mut || false)
        .map(|n| n.unwrap_or(0)) // a never-true predicate cannot return None
}

/// Acquires a full line, probing after each byte budget, carried across calls.
/// Returns `None` on hangup, `Some(0)` at EOF, or `Some(n)` for a full/partial
/// line. Bounds consumption even when `fill_buf` exposes more than the budget.
/// Probes at the boundary before decoding, including when it ends on a newline.
/// A blocking `fill_buf`, buffered prefetch and completed-line parsing are not
/// interruptible. Invalid UTF-8 is decoded only after full acquisition, preserving
/// characters split across chunks. Retries interrupted reads like `read_until`.
fn read_line_lossy_interruptible<R: BufRead>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    line: &mut String,
    until_probe: &mut usize,
    reader_gone: &mut impl FnMut() -> bool,
) -> io::Result<Option<usize>> {
    buf.clear();
    line.clear();
    loop {
        if *until_probe == 0 {
            if reader_gone() {
                return Ok(None);
            }
            *until_probe = LINE_PROBE_BYTES;
        }
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let available = &available[..available.len().min(*until_probe)];
        let n = available
            .iter()
            .position(|&b| b == b'\n')
            .map_or(available.len(), |i| i + 1);
        let complete = n > 0 && available[n - 1] == b'\n';
        buf.extend_from_slice(&available[..n]);
        reader.consume(n);
        *until_probe -= n;
        // Even a newline at the budget boundary must be probed before decoding.
        if *until_probe == 0 {
            if reader_gone() {
                return Ok(None);
            }
            *until_probe = LINE_PROBE_BYTES;
        }
        if n == 0 || complete {
            line.push_str(&String::from_utf8_lossy(buf));
            return Ok(Some(buf.len()));
        }
    }
}

/// Reads every existing line, passing matches to the shared sink. With a limit,
/// the sink retains only the most recent N matches across scans; without it,
/// matches stream out as they are read. Returns the byte offset just past the last
/// newline-terminated line, the completed-line count, and a bounded malformed-line summary.
/// With `follow`, a trailing partial line is left pending without a warning:
/// `--follow` reads it from that offset once it is complete. A one-shot scan has
/// nothing to complete it, so it renders or counts that line immediately.
/// Stops at the first write to a closed pipe or when `reader_gone` reports hangup,
/// checked before reading and every [`HANGUP_PROBE_LINES`] lines thereafter.
/// The cadence counts every line, regardless of parsing or filtering. Acquisition
/// also probes every [`LINE_PROBE_BYTES`] bytes, across and within lines.
fn scan_backlog<R: BufRead, W: Write>(
    reader: &mut R,
    filter: &Filter,
    follow: bool,
    sink: &mut BacklogSink<'_, W>,
    mut reader_gone: impl FnMut() -> bool,
) -> Result<Backlog> {
    let mut pos = 0u64;
    let mut skipped = SkippedLines::default();
    let mut lines = 0u64;
    let mut buf = Vec::new();
    let mut line = String::new();
    let mut until_probe = 0;
    let mut bytes_until_probe = LINE_PROBE_BYTES;
    loop {
        if until_probe == 0 {
            if reader_gone() {
                return Ok(Backlog::ReaderGone);
            }
            until_probe = HANGUP_PROBE_LINES;
        }
        until_probe -= 1;
        let Some(n) = read_line_lossy_interruptible(
            reader,
            &mut buf,
            &mut line,
            &mut bytes_until_probe,
            &mut reader_gone,
        )?
        else {
            return Ok(Backlog::ReaderGone);
        };
        if n == 0 {
            break;
        }
        let complete = line.ends_with('\n');
        if complete {
            pos += n as u64;
            lines += 1;
        } else if follow {
            break; // a partial last line: left for the follow loop to print once
        }
        match parse_line(&line, filter, sink.format) {
            Line::Blank | Line::Filtered => {}
            Line::Malformed => skipped.record(if complete { lines } else { lines + 1 }),
            Line::Match(rendered) => {
                if !sink.emit(rendered)? {
                    return Ok(Backlog::ReaderGone);
                }
            }
        }
    }
    Ok(Backlog::Complete {
        pos,
        lines,
        skipped,
    })
}

/// Single-file test adapter for the backlog parser and output behavior.
#[cfg(test)]
fn emit_backlog<R: BufRead, W: Write>(
    reader: &mut R,
    filter: &Filter,
    format: Format,
    limit: Option<usize>,
    follow: bool,
    out: &mut W,
    reader_gone: impl FnMut() -> bool,
) -> Result<Backlog> {
    let mut sink = BacklogSink {
        format,
        limit,
        ring: VecDeque::new(),
        out,
    };
    let result = scan_backlog(reader, filter, follow, &mut sink, reader_gone)?;
    if result == Backlog::ReaderGone || !sink.finish()? {
        return Ok(Backlog::ReaderGone);
    }
    Ok(result)
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
/// tick and periodically during a drain, so nonmatching appends also stop).
/// Resumes from the end when the file is truncated or replaced, without replaying
/// its contents.
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
        let Drain::Complete { skipped } =
            drain_appended(path, filter, format, &mut tail, out, &mut reader_gone)?
        else {
            return Ok(());
        };
        warn_skipped(path, &skipped);
        std::thread::sleep(FOLLOW_POLL);
    }
}

/// The outcome of scanning appended records.
#[derive(Debug, PartialEq, Eq)]
enum Drain {
    /// Reached EOF or a partial line, with a summary of unparseable complete lines.
    Complete { skipped: SkippedLines },
    /// The output reader exited; no further drain is needed.
    ReaderGone,
}

/// Reads and emits any complete lines appended past `tail.pos`, advancing the
/// tail. Skips to the observed end if the file was replaced (its identity changed)
/// or shrank or changed its checkpoint (truncation/rewrite). An absent file or
/// unchanged file with no complete appends produces no output.
/// This also applies to replacement during the initial backlog scan.
/// Returns the count and bounded positions of unparseable complete lines on
/// completion, or reader gone.
/// Checks `reader_gone` before reading and every [`HANGUP_PROBE_LINES`] lines,
/// counting blank, malformed and filtered lines alike.
/// A trailing partial line (no newline yet) is left for the next call.
fn drain_appended<W: Write>(
    path: &Path,
    filter: &Filter,
    format: Format,
    tail: &mut Tail,
    out: &mut W,
    reader_gone: impl FnMut() -> bool,
) -> Result<Drain> {
    let Some(file) = follow_file(
        path,
        File::open(path),
        &mut tail.open_failure_reported,
        &mut io::stderr().lock(),
    ) else {
        return Ok(Drain::Complete {
            skipped: SkippedLines::default(),
        });
    };
    drain_opened(file, filter, format, tail, out, reader_gone)
}

/// Handles a follow open result without altering the saved cursor or file identity.
/// Missing files are quiet. Report only the first other error until a successful
/// open, even if subsequent errors differ, to bound diagnostics during an outage.
fn follow_file<W: Write>(
    path: &Path,
    opened: io::Result<File>,
    reported: &mut bool,
    err: &mut W,
) -> Option<File> {
    match opened {
        Ok(file) => {
            *reported = false;
            Some(file)
        }
        Err(e) => {
            if e.kind() != io::ErrorKind::NotFound && !*reported {
                *reported = true;
                // Best effort: a closed stderr must not stop following.
                let _ = writeln!(
                    err,
                    "warning: failed to open log file {} while following: {e}; retrying",
                    path.display()
                );
            }
            None
        }
    }
}

/// Drains a successfully opened file using the existing append/replacement policy.
fn drain_opened<W: Write>(
    file: File,
    filter: &Filter,
    format: Format,
    tail: &mut Tail,
    out: &mut W,
    mut reader_gone: impl FnMut() -> bool,
) -> Result<Drain> {
    // Identity and length come from the handle that is read, not from the path.
    let len = file.metadata().map_or(tail.pos, |m| m.len());
    let id = file_id(&file);
    // An identity that cannot be read says nothing about replacement: keep the saved
    // one, and do not take a first successful read after a failure for a change.
    let replaced = matches!((id, tail.id), (Some(now), Some(saved)) if now != saved);
    let previous_pos = tail.pos;
    let rewritten =
        !tail.checkpoint.is_empty() && read_checkpoint(&file, tail.pos)? != tail.checkpoint;
    if replaced || len < tail.pos || rewritten {
        // Prune retains records already seen, and identity cannot tell it apart
        // from rotation. Never replay replacement contents, including records
        // written before this poll; follow only subsequent appends.
        tail.lines = count_newlines(&file, len)?;
        tail.pos = len;
    }
    tail.id = id.or(tail.id);
    let mut skipped = SkippedLines::default();
    if len > tail.pos {
        let mut reader = BufReader::new(&file);
        reader.seek(SeekFrom::Start(tail.pos))?;
        let mut buf = Vec::new();
        let mut line = String::new();
        let mut until_probe = 0;
        loop {
            if until_probe == 0 {
                if reader_gone() {
                    return Ok(Drain::ReaderGone);
                }
                until_probe = HANGUP_PROBE_LINES;
            }
            until_probe -= 1;
            let n = read_line_lossy(&mut reader, &mut buf, &mut line)?;
            if n == 0 || !line.ends_with('\n') {
                break; // EOF or partial trailing line — wait for more
            }
            tail.pos += n as u64;
            tail.lines += 1;
            let parsed = parse_line(&line, filter, format);
            if matches!(parsed, Line::Match(_) | Line::Filtered) {
                warn_unmatched_terms(filter, true);
            }
            match parsed {
                Line::Malformed => skipped.record(tail.lines),
                Line::Match(rendered) => {
                    writeln!(out, "{rendered}")?;
                    out.flush()?;
                }
                Line::Blank | Line::Filtered => {}
            }
        }
    }
    if tail.pos != previous_pos || replaced || rewritten {
        tail.checkpoint = read_checkpoint(&file, tail.pos)?;
    }
    // Keep the old inode pinned until the new handle has been read, including
    // idle polls. An absent path above leaves the previous handle alive.
    #[cfg(unix)]
    {
        tail.file = Some(file);
    }
    Ok(Drain::Complete { skipped })
}

/// Reads a bounded raw-byte checkpoint immediately before `pos` using portable
/// seek/read operations. A concurrent shrink may return fewer bytes, which also
/// differs from a saved checkpoint. File reads are not an atomic snapshot.
fn read_checkpoint(mut file: &File, pos: u64) -> io::Result<Vec<u8>> {
    let start = pos.saturating_sub(TAIL_CHECKPOINT_BYTES as u64);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity((pos - start) as usize);
    file.take(pos - start).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Counts physical line boundaries up to the observed replacement end, without
/// parsing or replaying its records or buffering an arbitrarily large line.
#[allow(clippy::naive_bytecount)] // Replacement-only scans do not warrant a bytecount dependency.
fn count_newlines(mut file: &File, len: u64) -> io::Result<u64> {
    file.seek(SeekFrom::Start(0))?;
    let mut reader = BufReader::new(file.take(len));
    let mut lines = 0;
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return Ok(lines);
        }
        lines += buf.iter().filter(|&&byte| byte == b'\n').count() as u64;
        let consumed = buf.len();
        reader.consume(consumed);
    }
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

/// Parses one raw line and renders it when it matches the filter. If a dead
/// writer left a partial prefix, recover the last complete log-record suffix.
/// Filters and JSON output see only that suffix, never the damaged prefix.
fn parse_line(line: &str, filter: &Filter, format: Format) -> Line {
    let raw = line.trim_end_matches(['\n', '\r']);
    if raw.trim().is_empty() {
        return Line::Blank;
    }
    let Some((rec, raw)) = parse_record(raw) else {
        return Line::Malformed;
    };
    if filter.matches(&rec, raw) {
        Line::Match(format::render(&rec, raw, format))
    } else {
        Line::Filtered
    }
}

/// Tries the intact line first, then object-start suffixes from right to left.
/// Full deserialization avoids mistaking nested objects or string braces for a
/// record boundary: a candidate must contain one complete log record and no
/// trailing JSON. Return the original suffix so JSON output preserves its bytes.
fn parse_record(raw: &str) -> Option<(LogRecord, &str)> {
    if let Ok(rec) = serde_json::from_str(raw) {
        return Some((rec, raw));
    }
    for (start, _) in raw.rmatch_indices('{') {
        if start == 0 {
            continue; // already tried the whole line
        }
        let suffix = &raw[start..];
        if let Ok(rec) = serde_json::from_str(suffix) {
            return Some((rec, suffix));
        }
    }
    None
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
    /// Builds an expected bounded summary from known malformed positions.
    fn skipped_lines(positions: &[u64]) -> SkippedLines {
        let mut skipped = SkippedLines::default();
        for &position in positions {
            skipped.record(position);
        }
        skipped
    }

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

    /// Exercises the production open-result handling and drain with captured stderr.
    fn poll_open_result<W: Write>(
        path: &Path,
        opened: io::Result<File>,
        tail: &mut Tail,
        out: &mut Vec<u8>,
        err: &mut W,
    ) {
        if let Some(file) = follow_file(path, opened, &mut tail.open_failure_reported, err) {
            assert!(matches!(
                drain_opened(file, &empty_filter(), Format::Json, tail, out, || false).unwrap(),
                Drain::Complete { .. }
            ));
        }
    }

    #[test]
    fn follow_open_failures_recover_and_warn_again_without_losing_the_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        let initial = sample_lines();
        std::fs::write(&path, &initial).unwrap();
        let mut tail = Tail::default();
        let mut out = Vec::new();
        let mut err = Vec::new();
        poll_open_result(&path, File::open(&path), &mut tail, &mut out, &mut err);
        assert_eq!(out, initial.as_bytes());
        out.clear();
        let saved = (tail.pos, tail.lines, tail.id, tail.checkpoint.clone());
        #[cfg(unix)]
        let saved_fd = {
            use std::os::fd::AsRawFd;
            tail.file.as_ref().unwrap().as_raw_fd()
        };
        for kind in [
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::Other,
            io::ErrorKind::NotFound,
            io::ErrorKind::PermissionDenied,
        ] {
            poll_open_result(
                &path,
                Err(io::Error::new(kind, "injected access failure")),
                &mut tail,
                &mut out,
                &mut err,
            );
            assert_eq!(
                (tail.pos, tail.lines, tail.id, tail.checkpoint.clone()),
                saved
            );
            #[cfg(unix)]
            {
                use std::os::fd::AsRawFd;
                assert_eq!(tail.file.as_ref().unwrap().as_raw_fd(), saved_fd);
            }
            assert!(out.is_empty());
        }
        let warning = String::from_utf8(err.clone()).unwrap();
        assert_eq!(warning.lines().count(), 1);
        assert!(warning.contains(&path.display().to_string()));
        assert!(warning.contains("injected access failure"));
        assert!(warning.contains("retrying"));

        // A successful open with only a partial append resets suppression without
        // advancing the cursor; finishing that line later must emit it once.
        let appended = r#"{"id":"after-recovery","kind":"http"}"#;
        let mut writer = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        write!(writer, "{appended}").unwrap();
        poll_open_result(&path, File::open(&path), &mut tail, &mut out, &mut err);
        assert!(!tail.open_failure_reported);
        assert_eq!(tail.pos, saved.0);
        assert!(out.is_empty());
        poll_open_result(
            &path,
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "later failure",
            )),
            &mut tail,
            &mut out,
            &mut err,
        );
        assert_eq!(String::from_utf8(err.clone()).unwrap().lines().count(), 2);
        assert!(String::from_utf8(err.clone())
            .unwrap()
            .contains("later failure"));
        writeln!(writer).unwrap();
        poll_open_result(&path, File::open(&path), &mut tail, &mut out, &mut err);
        poll_open_result(&path, File::open(&path), &mut tail, &mut out, &mut err);
        assert_eq!(out, format!("{appended}\n").as_bytes());
        assert_eq!(tail.lines, saved.1 + 1);
        assert_eq!(String::from_utf8(err).unwrap().lines().count(), 2);
    }

    #[test]
    fn follow_missing_start_stays_quiet_and_reads_created_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        let mut tail = Tail::default();
        let mut out = Vec::new();
        let mut err = Vec::new();
        for _ in 0..3 {
            poll_open_result(&path, File::open(&path), &mut tail, &mut out, &mut err);
        }
        assert!(err.is_empty());
        assert!(out.is_empty());
        assert!(!tail.open_failure_reported);
        let initial = sample_lines();
        std::fs::write(&path, &initial).unwrap();
        poll_open_result(&path, File::open(&path), &mut tail, &mut out, &mut err);
        assert_eq!(out, initial.as_bytes());
        assert!(err.is_empty());
    }

    #[test]
    fn follow_rewrite_during_open_failure_skips_existing_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, sample_lines()).unwrap();
        let mut tail = Tail::default();
        let mut out = Vec::new();
        let mut err = Vec::new();
        poll_open_result(&path, File::open(&path), &mut tail, &mut out, &mut err);
        out.clear();
        poll_open_result(
            &path,
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied")),
            &mut tail,
            &mut out,
            &mut err,
        );
        let replacement = "{\"id\":\"replacement\"}\n";
        std::fs::write(&path, replacement).unwrap();
        poll_open_result(&path, File::open(&path), &mut tail, &mut out, &mut err);
        assert!(out.is_empty());
        assert_eq!(tail.pos, replacement.len() as u64);
        assert_eq!(tail.lines, 1);
        let appended = "{\"id\":\"new-append\"}\n";
        let mut writer = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        write!(writer, "{appended}").unwrap();
        poll_open_result(&path, File::open(&path), &mut tail, &mut out, &mut err);
        assert_eq!(out, appended.as_bytes());
        assert_eq!(tail.lines, 2);
    }

    #[test]
    fn follow_closed_stderr_does_not_prevent_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        let mut tail = Tail::default();
        let mut out = Vec::new();
        let mut err = PipeWriter::failing_with(io::ErrorKind::BrokenPipe, 0);
        poll_open_result(
            &path,
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied")),
            &mut tail,
            &mut out,
            &mut err,
        );
        assert!(tail.open_failure_reported);
        let attempts = err.attempts;
        poll_open_result(
            &path,
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied")),
            &mut tail,
            &mut out,
            &mut err,
        );
        assert_eq!(err.attempts, attempts);
        std::fs::write(&path, sample_lines()).unwrap();
        poll_open_result(&path, File::open(&path), &mut tail, &mut out, &mut err);
        assert!(!tail.open_failure_reported);
        assert_eq!(out, sample_lines().as_bytes());
    }

    #[test]
    fn follow_truncation_does_not_reobserve_retained_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        let raw = r#"{"id":"one","kind":"drivemutation","timestamp":"2020-01-01T00:00:00Z","context":{"status":"blocked"}}"#;
        let f = Filter::build(FilterInput {
            status: Some("blokced"),
            query: &["servce:drive".to_string()],
            ..filter_input()
        })
        .unwrap();
        std::fs::write(&path, format!("{raw}\n{raw}\n")).unwrap();
        let mut tail = Tail::default();
        let mut out = Vec::new();
        drain_appended(&path, &f, Format::Json, &mut tail, &mut out, || false).unwrap();
        std::fs::write(&path, format!("{raw}\n")).unwrap();
        drain_appended(&path, &f, Format::Json, &mut tail, &mut out, || false).unwrap();
        assert!(f.unknown_field_warnings(true)[0].contains("2 records scanned"));
        assert!(f.unseen_status_warnings(true)[0].contains("2 drivemutation records scanned"));
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{raw}").unwrap();
        drain_appended(&path, &f, Format::Json, &mut tail, &mut out, || false).unwrap();
        assert!(f.unknown_field_warnings(true)[0].contains("3 records scanned"));
        assert!(f.unseen_status_warnings(true)[0].contains("3 drivemutation records scanned"));
        assert!(f.pending_follow_warnings().is_empty());
        assert!(out.is_empty());
    }

    #[test]
    fn custom_log_name_discovers_backups_even_in_a_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("custom.requests");
        let backup = dir.path().join("custom.requests.7");
        std::fs::write(&backup, "").unwrap();
        std::fs::write(dir.path().join("log.jsonl.8"), "").unwrap();
        assert_eq!(
            backlog_paths(&live, true).unwrap(),
            vec![backup, live.clone()]
        );
        assert_eq!(backlog_paths(&live, false).unwrap(), vec![live]);
        let missing = dir.path().join("absent/log.jsonl");
        assert_eq!(backlog_paths(&missing, true).unwrap(), vec![missing]);
    }

    #[test]
    fn rotated_discovery_reports_an_invalid_log_directory() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("not-a-directory");
        std::fs::write(&parent, "").unwrap();
        let err = backlog_paths(&parent.join("log.jsonl"), true).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("Failed to read log directory {}", parent.display())
        );
        assert!(err.downcast_ref::<io::Error>().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn backup_generation_accepts_non_utf8_prefixes_but_rejects_non_utf8_suffixes() {
        use std::os::unix::ffi::OsStrExt;

        let prefix = OsStr::from_bytes(b"requests-\xff.jsonl.");
        assert_eq!(
            backup_generation(OsStr::from_bytes(b"requests-\xff.jsonl.7"), prefix),
            Some(7)
        );
        assert_eq!(
            backup_generation(OsStr::from_bytes(b"requests-\xff.jsonl.\xff"), prefix),
            None
        );
    }

    #[test]
    fn backup_generation_accepts_only_canonical_positive_numbers_with_the_exact_prefix() {
        let prefix = OsStr::new("log.jsonl.");
        assert_eq!(
            backup_generation(OsStr::new("log.jsonl.10"), prefix),
            Some(10)
        );
        for filename in [
            "other.jsonl.1",
            "log.jsonl.0",
            "log.jsonl.01",
            "log.jsonl.+1",
            "log.jsonl.-1",
            "log.jsonl.tmp",
            "log.jsonl.4294967296",
        ] {
            assert_eq!(
                backup_generation(OsStr::new(filename), prefix),
                None,
                "{filename}"
            );
        }
    }

    // Linux permits these filenames; macOS filesystems reject them with EILSEQ.
    #[cfg(target_os = "linux")]
    #[test]
    fn rotated_discovery_ignores_non_utf8_suffixes_and_preserves_non_utf8_log_names() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let dir = tempfile::tempdir().unwrap();
        let live = dir
            .path()
            .join(OsString::from_vec(b"requests-\xff.jsonl".to_vec()));
        let backup = dir
            .path()
            .join(OsString::from_vec(b"requests-\xff.jsonl.1".to_vec()));
        let unrelated = dir
            .path()
            .join(OsString::from_vec(b"requests-\xff.jsonl.\xff".to_vec()));
        std::fs::write(&backup, "").unwrap();
        std::fs::write(unrelated, "").unwrap();
        assert_eq!(backlog_paths(&live, true).unwrap(), vec![backup, live]);
    }

    #[cfg(unix)]
    #[test]
    fn rotated_scan_reports_an_unreadable_backup_instead_of_silently_skipping_it() {
        use std::os::unix::fs::PermissionsExt;

        // Root bypasses the permissions used to force this open failure.
        crate::test_support::skip_as_root!();
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("log.jsonl");
        let backup = dir.path().join("log.jsonl.1");
        std::fs::write(&backup, sample_lines()).unwrap();
        std::fs::write(&live, sample_lines()).unwrap();
        std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o000)).unwrap();
        let mut out = Vec::new();
        let mut sink = BacklogSink {
            format: Format::Json,
            limit: None,
            ring: VecDeque::new(),
            out: &mut out,
        };
        let err = scan_files(&live, &empty_filter(), false, true, &mut sink, || false).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("Failed to open log file {}", backup.display())
        );
        assert_eq!(
            err.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(out.is_empty());
    }

    #[test]
    fn rotated_scan_exits_cleanly_when_the_pipe_closes_during_limit_replay() {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("log.jsonl");
        std::fs::write(dir.path().join("log.jsonl.1"), sample_lines()).unwrap();
        std::fs::write(&live, sample_lines()).unwrap();
        let mut out = PipeWriter::failing_with(io::ErrorKind::BrokenPipe, 0);
        let mut sink = BacklogSink {
            format: Format::Json,
            limit: Some(3),
            ring: VecDeque::new(),
            out: &mut out,
        };
        let mut probes = 0;
        let result = scan_files(&live, &empty_filter(), false, true, &mut sink, || {
            probes += 1;
            false
        })
        .unwrap();
        assert!(result.is_none());
        // Both files were scanned before the first buffered write found the closed pipe.
        assert_eq!(probes, 2);
        assert_eq!(sink.ring.len(), 3);
        assert_eq!(out.attempts, 1);
    }

    #[test]
    fn rotated_backlog_follow_uses_one_ring_and_only_the_live_offset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(dir.path().join("log.jsonl.1"), UNTERMINATED).unwrap();
        let complete = sample_lines();
        std::fs::write(&path, format!("{complete}{UNTERMINATED}")).unwrap();
        let filter = empty_filter();
        let mut out = Vec::new();
        let mut sink = BacklogSink {
            format: Format::Json,
            limit: Some(6),
            ring: VecDeque::new(),
            out: &mut out,
        };
        let mut tail = scan_files(&path, &filter, true, true, &mut sink, || false)
            .unwrap()
            .unwrap();
        assert_eq!(tail.pos, complete.len() as u64);
        assert_eq!(
            String::from_utf8_lossy(&out),
            format!("{UNTERMINATED}\n{complete}")
        );
        // Complete the live partial record; it is emitted once, without rescanning archives.
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"\n")
            .unwrap();
        drain_appended(&path, &filter, Format::Json, &mut tail, &mut out, || false).unwrap();
        drain_appended(&path, &filter, Format::Json, &mut tail, &mut out, || false).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out),
            format!("{UNTERMINATED}\n{complete}{UNTERMINATED}\n")
        );
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
                lines: 5,
                skipped: SkippedLines::default()
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
                lines: 5,
                skipped: SkippedLines::default()
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
                lines: input.lines().count() as u64,
                skipped: SkippedLines::default()
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
        let Backlog::Complete { pos, lines, .. } = emit_backlog(
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
        let mut tail = Tail {
            open_failure_reported: false,
            pos,
            lines,
            id,
            checkpoint: Vec::new(),
            #[cfg(unix)]
            file: None,
        };

        // The writer has not finished: the follow loop prints nothing yet.
        let mut out = Vec::new();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
        assert!(out.is_empty());

        // The writer finishes the line.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f).unwrap();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();

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
                lines: 5,
                skipped: skipped_lines(&[1, 3, 4])
            }
        );
    }

    #[test]
    fn backlog_counts_only_unrecoverable_lines_including_an_unterminated_tail() {
        let clean = format!("{GOOD}\n{GOOD}\n");
        let (_, result) = scan(clean.as_bytes(), &empty_filter(), None);
        assert_eq!(
            result,
            Backlog::Complete {
                pos: clean.len() as u64,
                lines: 2,
                skipped: SkippedLines::default()
            }
        );

        let blanks = format!("\n{GOOD}\n\r\n  \t\n\n");
        let (_, result) = scan(blanks.as_bytes(), &empty_filter(), None);
        assert_eq!(
            result,
            Backlog::Complete {
                pos: blanks.len() as u64,
                lines: 5,
                skipped: SkippedLines::default()
            }
        );

        // One-shot scans report an incomplete final record too.
        let partial = format!("{GOOD}\n{{\"id\":\"2\",\"ki");
        let (text, result) = scan(partial.as_bytes(), &empty_filter(), None);
        assert_eq!(text, format!("{GOOD}\n"));
        assert_eq!(
            result,
            Backlog::Complete {
                pos: GOOD.len() as u64 + 1,
                lines: 1,
                skipped: skipped_lines(&[2])
            }
        );
    }

    #[test]
    fn backlog_counts_unparseable_lines_the_filter_and_limit_would_hide() {
        let input = format!("junk\n{GOOD}\njunk\n{GOOD}\njunk\n");
        let skipped = |result| match result {
            Backlog::Complete { skipped, .. } => skipped.count,
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
    fn recovery_preserves_nested_records_and_filters_only_the_suffix() {
        let good = r#"{ "id":"résumé", "kind":"http", "context":{"note":"literal { brace"} }"#;
        let fused = format!("{{partial{good}\r\n");
        assert_eq!(
            parse_line(&fused, &empty_filter(), Format::Json),
            Line::Match(good.to_string())
        );
        let filter = Filter::build(FilterInput {
            grep: Some("partial"),
            ..filter_input()
        })
        .unwrap();
        assert_eq!(parse_line(&fused, &filter, Format::Json), Line::Filtered);
        let filter = Filter::build(FilterInput {
            id: Some("résumé"),
            ..filter_input()
        })
        .unwrap();
        assert_eq!(
            parse_line(&fused, &filter, Format::Json),
            Line::Match(good.to_string())
        );
        for raw in [
            "{partial",
            "{partial{\"kind\":3}",
            "{partial{\"id\":\"x\"} trailing",
        ] {
            assert_eq!(
                parse_line(raw, &empty_filter(), Format::Json),
                Line::Malformed
            );
        }
    }

    #[test]
    fn backlog_recovers_fused_records_with_and_without_a_limit() {
        let input = format!("{{partial{GOOD}\n{{broken{UNTERMINATED}\n");
        for (limit, expected) in [
            (None, format!("{GOOD}\n{UNTERMINATED}\n")),
            (Some(1), format!("{UNTERMINATED}\n")),
        ] {
            let (text, result) = scan(input.as_bytes(), &empty_filter(), limit);
            assert_eq!(text, expected);
            assert_eq!(
                result,
                Backlog::Complete {
                    pos: input.len() as u64,
                    lines: input.lines().count() as u64,
                    skipped: SkippedLines::default()
                }
            );
        }
        let (text, result) = scan(format!("{{partial{GOOD}").as_bytes(), &empty_filter(), None);
        assert_eq!(text, format!("{GOOD}\n"));
        assert_eq!(
            result,
            Backlog::Complete {
                pos: 0,
                lines: 0,
                skipped: SkippedLines::default()
            }
        );
    }

    #[test]
    fn follow_recovers_after_a_partial_append_and_emits_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, "{partial").unwrap();
        let mut reader = BufReader::new(File::open(&path).unwrap());
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
        assert_eq!(
            result,
            Backlog::Complete {
                pos: 0,
                lines: 0,
                skipped: SkippedLines::default()
            }
        );
        let mut tail = tail_at_start();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
        assert!(out.is_empty());
        assert_eq!(tail.pos, 0);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{GOOD}").unwrap();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), format!("{GOOD}\n"));
        assert_eq!(tail.pos, file.metadata().unwrap().len());
    }

    #[test]
    fn skipped_warning_names_the_count_and_the_path() {
        let path = Path::new("/state/log.jsonl");
        assert_eq!(skipped_warning(path, &SkippedLines::default()), None);
        assert_eq!(
            skipped_warning(path, &skipped_lines(&[12])).as_deref(),
            Some("warning: skipped 1 unparseable line in /state/log.jsonl (lines 12)")
        );
        assert_eq!(
            skipped_warning(path, &skipped_lines(&[12, 40, 41])).as_deref(),
            Some("warning: skipped 3 unparseable lines in /state/log.jsonl (lines 12, 40, 41)")
        );
    }

    #[test]
    fn skipped_warning_caps_positions_but_preserves_the_total() {
        let path = Path::new("/state/log.jsonl");
        for (positions, suffix) in [
            (vec![2, 4, 6, 8, 10], "lines 2, 4, 6, 8, 10"),
            (vec![2, 4, 6, 8, 10, 12], "lines 2, 4, 6, 8, 10 and 1 more"),
            ((1..=100).collect(), "lines 1, 2, 3, 4, 5 and 95 more"),
        ] {
            let skipped = skipped_lines(&positions);
            assert_eq!(skipped.positions.len(), 5);
            assert_eq!(
                skipped_warning(path, &skipped).unwrap(),
                format!(
                    "warning: skipped {} unparseable lines in /state/log.jsonl ({suffix})",
                    positions.len()
                )
            );
        }
    }

    #[test]
    fn scan_files_passes_live_line_numbers_to_follow() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, format!("\n{GOOD}\npartial")).unwrap();
        let mut out = Vec::new();
        let mut sink = BacklogSink {
            format: Format::Json,
            limit: Some(0),
            ring: VecDeque::new(),
            out: &mut out,
        };
        let mut tail = scan_files(&path, &empty_filter(), true, false, &mut sink, || false)
            .unwrap()
            .unwrap();
        assert_eq!(tail.lines, 2);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"\n\ninvalid\n").unwrap();
        assert_eq!(
            drain_appended(
                &path,
                &empty_filter(),
                Format::Json,
                &mut tail,
                &mut out,
                || false
            )
            .unwrap(),
            Drain::Complete {
                skipped: skipped_lines(&[3, 5])
            }
        );
        file.write_all(b"more\n").unwrap();
        assert_eq!(
            drain_appended(
                &path,
                &empty_filter(),
                Format::Json,
                &mut tail,
                &mut out,
                || false
            )
            .unwrap(),
            Drain::Complete {
                skipped: skipped_lines(&[6])
            }
        );
    }

    #[test]
    fn follow_line_numbers_restart_after_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, format!("{GOOD}\n{GOOD}\n{GOOD}\n")).unwrap();
        let mut tail = tail_at_start();
        let mut out = Vec::new();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
        std::fs::write(&path, b"\n\r\npartial").unwrap();
        assert_eq!(
            drain_appended(
                &path,
                &empty_filter(),
                Format::Json,
                &mut tail,
                &mut out,
                || false
            )
            .unwrap(),
            Drain::Complete {
                skipped: SkippedLines::default()
            }
        );
        assert_eq!(tail.lines, 2);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"bad\ninvalid\n").unwrap();
        assert_eq!(
            drain_appended(
                &path,
                &empty_filter(),
                Format::Json,
                &mut tail,
                &mut out,
                || false
            )
            .unwrap(),
            Drain::Complete {
                skipped: skipped_lines(&[3, 4])
            }
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn follow_line_numbers_restart_after_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, format!("{GOOD}\n")).unwrap();
        let mut tail = tail_at_start();
        let mut out = Vec::new();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
        replace_by_rename(&path, &format!("{GOOD}\n\n{GOOD}\n{GOOD}\n"));
        assert_eq!(
            drain_appended(
                &path,
                &empty_filter(),
                Format::Json,
                &mut tail,
                &mut out,
                || false
            )
            .unwrap(),
            Drain::Complete {
                skipped: SkippedLines::default()
            }
        );
        assert_eq!(tail.lines, 4);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"invalid\n").unwrap();
        assert_eq!(
            drain_appended(
                &path,
                &empty_filter(),
                Format::Json,
                &mut tail,
                &mut out,
                || false
            )
            .unwrap(),
            Drain::Complete {
                skipped: skipped_lines(&[5])
            }
        );
    }

    #[test]
    fn drain_appended_counts_corrupt_lines_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, format!("junk\n\n \t\r\n{GOOD}\n\x00\n")).unwrap();
        let mut tail = tail_at_start();
        let mut out = Vec::new();
        assert_eq!(
            drain_appended(
                &path,
                &empty_filter(),
                Format::Json,
                &mut tail,
                &mut out,
                || false
            )
            .unwrap(),
            Drain::Complete {
                skipped: skipped_lines(&[1, 5])
            }
        );
        assert_eq!(String::from_utf8(out.clone()).unwrap(), format!("{GOOD}\n"));
        assert_eq!(
            drain_appended(
                &path,
                &empty_filter(),
                Format::Json,
                &mut tail,
                &mut out,
                || false
            )
            .unwrap(),
            Drain::Complete {
                skipped: SkippedLines::default()
            }
        );
        assert_eq!(String::from_utf8(out).unwrap(), format!("{GOOD}\n"));
    }

    #[test]
    fn drain_appended_counts_only_complete_corruption_even_with_a_filter() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, format!("{GOOD}\npartial")).unwrap();
        let filter = Filter::build(FilterInput {
            service: Some("drive"),
            ..filter_input()
        })
        .unwrap();
        let mut tail = tail_at_start();
        let mut out = Vec::new();
        assert_eq!(
            drain_appended(&path, &filter, Format::Json, &mut tail, &mut out, || false).unwrap(),
            Drain::Complete {
                skipped: SkippedLines::default()
            }
        );
        assert_eq!(tail.pos, (GOOD.len() + 1) as u64);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"\n\xff\n").unwrap();
        assert_eq!(
            drain_appended(&path, &filter, Format::Json, &mut tail, &mut out, || false).unwrap(),
            Drain::Complete {
                skipped: skipped_lines(&[2, 3])
            }
        );
        assert_eq!(
            drain_appended(&path, &filter, Format::Json, &mut tail, &mut out, || false).unwrap(),
            Drain::Complete {
                skipped: SkippedLines::default()
            }
        );
        assert!(out.is_empty());
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
        assert!(run(&missing, &empty_filter(), Format::Json, None, false, false).is_ok());
    }

    #[test]
    fn run_reads_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, sample_lines()).unwrap();
        // Exercises the open + backlog path (output goes to captured stdout).
        assert!(run(&path, &empty_filter(), Format::Json, Some(2), false, false).is_ok());
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
    fn drain_probe_stops_no_match_appends_and_new_file_scans() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        let filter = Filter::build(FilterInput {
            service: Some("drive"),
            ..filter_input()
        })
        .unwrap();
        // A new file starts at byte 0; an ordinary append starts after the backlog.
        for prefix in [String::new(), format!("{GOOD}\n")] {
            std::fs::write(&path, &prefix).unwrap();
            let mut tail = tail_at_start();
            drain_appended(
                &path,
                &empty_filter(),
                Format::Json,
                &mut tail,
                &mut Vec::new(),
                || false,
            )
            .unwrap();
            assert_drain_probe_stops(&path, &filter, &mut tail, &format!("{GOOD}\n"));
        }
    }

    /// Appends a large batch and verifies a second probe stops at the shared cadence.
    fn assert_drain_probe_stops(path: &Path, filter: &Filter, tail: &mut Tail, line: &str) {
        let start = tail.pos;
        let start_lines = tail.lines;
        let batch = line.repeat(HANGUP_PROBE_LINES * 3);
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(batch.as_bytes())
            .unwrap();
        let mut out = Vec::new();
        let mut probes = 0;
        let result = drain_appended(path, filter, Format::Json, tail, &mut out, || {
            probes += 1;
            probes == 2
        })
        .unwrap();
        assert_eq!(result, Drain::ReaderGone);
        assert_eq!(probes, 2);
        assert_eq!(tail.pos, start + (line.len() * HANGUP_PROBE_LINES) as u64);
        assert_eq!(tail.lines, start_lines + HANGUP_PROBE_LINES as u64);
        assert!(tail.pos < std::fs::metadata(path).unwrap().len());
        assert!(out.is_empty());
    }

    #[test]
    fn drain_probe_counts_blank_and_malformed_lines() {
        for line in ["\n", "not json\n"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("log.jsonl");
            std::fs::write(&path, "").unwrap();
            assert_drain_probe_stops(&path, &empty_filter(), &mut tail_at_start(), line);
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn drain_probe_stops_appends_after_replacement_without_replaying() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, format!("{GOOD}\n")).unwrap();
        let mut tail = tail_at_start();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut Vec::new(),
            || false,
        )
        .unwrap();
        let replacement = format!("{GOOD}\n").repeat(HANGUP_PROBE_LINES * 3);
        replace_by_rename(&path, &replacement);
        let mut out = Vec::new();
        let result = drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
        assert_eq!(
            result,
            Drain::Complete {
                skipped: SkippedLines::default()
            }
        );
        assert_eq!(tail.pos, replacement.len() as u64);
        assert!(out.is_empty());
        let filter = Filter::build(FilterInput {
            service: Some("drive"),
            ..filter_input()
        })
        .unwrap();
        assert_drain_probe_stops(&path, &filter, &mut tail, &format!("{GOOD}\n"));
    }

    #[test]
    fn drain_probe_detects_a_closed_reader_before_reading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, sample_lines()).unwrap();
        let mut tail = tail_at_start();
        let mut out = Vec::new();
        let result = drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || true,
        )
        .unwrap();
        assert_eq!(result, Drain::ReaderGone);
        assert_eq!(tail.pos, 0);
        assert!(out.is_empty());
    }

    #[test]
    fn drain_probe_live_reader_preserves_output_and_partial_line_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        let batch = format!("{GOOD}\n").repeat(HANGUP_PROBE_LINES * 2);
        std::fs::write(&path, format!("{batch}{GOOD}")).unwrap();
        let mut tail = tail_at_start();
        let mut out = Vec::new();
        let mut probes = 0;
        let result = drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || {
                probes += 1;
                false
            },
        )
        .unwrap();
        assert_eq!(
            result,
            Drain::Complete {
                skipped: SkippedLines::default()
            }
        );
        assert_eq!(probes, 3);
        assert_eq!(tail.pos, batch.len() as u64);
        assert_eq!(String::from_utf8(out.clone()).unwrap(), batch);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"\n")
            .unwrap();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), format!("{batch}{GOOD}\n"));
        assert_eq!(tail.pos, std::fs::metadata(&path).unwrap().len());
    }

    #[test]
    fn follow_loop_exits_immediately_when_the_drain_reports_reader_gone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, "\n".repeat(HANGUP_PROBE_LINES * 3)).unwrap();
        let mut out = Vec::new();
        let mut probes = 0;
        follow_loop(
            &path,
            &empty_filter(),
            Format::Json,
            tail_at_start(),
            &mut out,
            || {
                probes += 1;
                // Outer probe, drain's first probe, then hangup during the drain.
                assert!(
                    probes <= 3,
                    "follow polled again after the drain detected hangup"
                );
                probes == 3
            },
        )
        .unwrap();
        assert_eq!(probes, 3);
        assert!(out.is_empty());
    }

    #[test]
    fn backlog_byte_probe_stops_inside_oversized_lines() {
        for terminated in [false, true] {
            let mut input = "x".repeat(LINE_PROBE_BYTES * 4);
            if terminated {
                input.push('\n');
            }
            for follow in [false, true] {
                // Cursor exposes the entire input; BufReader exposes small chunks.
                for capacity in [31, input.len()] {
                    let mut reader = BufReader::with_capacity(capacity, Cursor::new(&input));
                    let mut out = Vec::new();
                    let mut probes = 0;
                    let result = emit_backlog(
                        &mut reader,
                        &empty_filter(),
                        Format::Json,
                        Some(3),
                        follow,
                        &mut out,
                        || {
                            probes += 1;
                            probes == 2
                        },
                    )
                    .unwrap();
                    assert_eq!(result, Backlog::ReaderGone);
                    assert_eq!(reader.stream_position().unwrap() as usize, LINE_PROBE_BYTES);
                    assert_eq!(probes, 2);
                    assert!(out.is_empty());
                }
            }
        }
    }

    #[test]
    fn byte_probe_carries_budget_across_lines_and_precedes_decoding() {
        let input = format!("{}\n{}\n", "a".repeat(100), "b".repeat(LINE_PROBE_BYTES));
        let mut reader = Cursor::new(input.as_bytes());
        let mut buf = Vec::new();
        let mut line = String::new();
        let mut budget = LINE_PROBE_BYTES;
        assert_eq!(
            read_line_lossy_interruptible(
                &mut reader,
                &mut buf,
                &mut line,
                &mut budget,
                &mut || false,
            )
            .unwrap(),
            Some(101)
        );
        assert_eq!(budget, LINE_PROBE_BYTES - 101);
        assert_eq!(
            read_line_lossy_interruptible(
                &mut reader,
                &mut buf,
                &mut line,
                &mut budget,
                &mut || true,
            )
            .unwrap(),
            None
        );
        assert_eq!(reader.position() as usize, LINE_PROBE_BYTES);
        assert_eq!(buf.len(), LINE_PROBE_BYTES - 101);
        assert!(line.is_empty()); // the abandoned line was never decoded
    }

    #[test]
    fn exhausted_byte_budget_probes_before_acquisition() {
        for gone in [true, false] {
            let mut reader = Cursor::new(b"ok\n");
            let mut buf = Vec::new();
            let mut line = String::new();
            let mut budget = 0;
            let mut probes = 0;
            let result = read_line_lossy_interruptible(
                &mut reader,
                &mut buf,
                &mut line,
                &mut budget,
                &mut || {
                    probes += 1;
                    gone
                },
            )
            .unwrap();
            assert_eq!(probes, 1);
            if gone {
                assert_eq!(result, None);
                assert_eq!(reader.position(), 0);
            } else {
                assert_eq!(result, Some(3));
                assert_eq!(line, "ok\n");
                assert_eq!(budget, LINE_PROBE_BYTES - 3);
            }
        }
    }

    #[test]
    fn byte_probe_checks_newline_boundary_before_decoding() {
        let input = format!("{}\n", "x".repeat(LINE_PROBE_BYTES - 1));
        let mut reader = Cursor::new(input);
        let mut buf = Vec::new();
        let mut line = String::new();
        let mut budget = LINE_PROBE_BYTES;
        let result = read_line_lossy_interruptible(
            &mut reader,
            &mut buf,
            &mut line,
            &mut budget,
            &mut || true,
        )
        .unwrap();
        assert_eq!(result, None);
        assert!(line.is_empty());
        assert_eq!(reader.position() as usize, LINE_PROBE_BYTES);
    }

    #[test]
    fn oversized_valid_records_remain_intact_and_partial_follow_stays_pending() {
        let mut value: serde_json::Value =
            serde_json::from_str(sample_lines().lines().next().unwrap()).unwrap();
        value["url"] = serde_json::Value::String(format!("/{}é", "x".repeat(LINE_PROBE_BYTES * 2)));
        let raw = serde_json::to_string(&value).unwrap();
        for terminated in [false, true] {
            for follow in [false, true] {
                let input = format!("{raw}{}", if terminated { "\n" } else { "" });
                let mut reader = BufReader::with_capacity(7, Cursor::new(&input));
                let mut out = Vec::new();
                let mut probes = 0;
                let result = emit_backlog(
                    &mut reader,
                    &empty_filter(),
                    Format::Json,
                    None,
                    follow,
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
                        pos: if terminated { input.len() as u64 } else { 0 },
                        lines: u64::from(terminated),
                        skipped: SkippedLines::default(),
                    }
                );
                if follow && !terminated {
                    assert!(out.is_empty());
                } else {
                    assert_eq!(out, format!("{raw}\n").as_bytes());
                }
                assert_eq!(probes, 1 + input.len() / LINE_PROBE_BYTES);
            }
        }
    }

    #[test]
    fn lossy_reader_preserves_utf8_split_at_byte_budget_and_replaces_invalid_bytes() {
        let mut input = vec![b'x'; LINE_PROBE_BYTES - 1];
        input.extend_from_slice("é".as_bytes());
        input.extend_from_slice(b"\xff\n");
        let mut reader = BufReader::with_capacity(7, Cursor::new(&input));
        let mut buf = Vec::new();
        let mut line = String::new();
        assert_eq!(
            read_line_lossy(&mut reader, &mut buf, &mut line).unwrap(),
            input.len()
        );
        assert_eq!(line, String::from_utf8_lossy(&input));
        assert!(line.ends_with("é�\n"));
        assert_eq!(
            read_line_lossy(&mut reader, &mut buf, &mut line).unwrap(),
            0
        );
        assert!(line.is_empty());
    }

    /// Emits one read error, then serves bytes from a cursor.
    struct ErrorOnce {
        error: Option<io::ErrorKind>,
        input: Cursor<Vec<u8>>,
    }

    impl io::Read for ErrorOnce {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if let Some(error) = self.error.take() {
                return Err(io::Error::from(error));
            }
            self.input.read(buf)
        }
    }

    #[test]
    fn lossy_reader_retries_interrupted_reads_and_propagates_other_errors() {
        for kind in [io::ErrorKind::Interrupted, io::ErrorKind::PermissionDenied] {
            let mut reader = BufReader::new(ErrorOnce {
                error: Some(kind),
                input: Cursor::new(b"hello\n".to_vec()),
            });
            let mut buf = Vec::new();
            let mut line = String::new();
            let result = read_line_lossy(&mut reader, &mut buf, &mut line);
            if kind == io::ErrorKind::Interrupted {
                assert_eq!(result.unwrap(), 6);
                assert_eq!(line, "hello\n");
            } else {
                assert_eq!(result.unwrap_err().kind(), kind);
                assert!(line.is_empty());
            }
        }
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
                assert_eq!(input[..consumed].lines().count(), HANGUP_PROBE_LINES);
            }
        }
    }

    #[test]
    fn backlog_probe_counts_blank_and_malformed_lines() {
        for line in ["\n", "not json\n"] {
            let input = line.repeat(HANGUP_PROBE_LINES * 3);
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
            assert_eq!(reader.position() as usize, line.len() * HANGUP_PROBE_LINES);
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
                lines: input.lines().count() as u64,
                skipped: SkippedLines::default()
            }
        );
        assert_eq!(out, input.as_bytes());
        assert_eq!(
            probes,
            input.lines().count() / HANGUP_PROBE_LINES + 1 + input.len() / LINE_PROBE_BYTES
        );
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
        let tail = Tail::default();

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
        let tail = Tail::default();
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
        Tail::default()
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
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
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
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
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
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
        assert!(String::from_utf8(out).unwrap().is_empty());
        assert_eq!(tail.pos, pos2, "partial line does not advance the position");

        // Truncation skips existing contents and follows subsequent appends.
        std::fs::write(&path, "{\"id\":\"x\",\"kind\":\"http\"}\n").unwrap();
        let mut out = Vec::new();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
        assert!(out.is_empty());
        assert_eq!(tail.pos, std::fs::metadata(&path).unwrap().len());
        assert_follows_next_append(&path, &mut tail);

        // A missing file is a no-op that preserves the tail.
        let missing = dir.path().join("gone.jsonl");
        let mut kept = Tail {
            open_failure_reported: false,
            pos: 7,
            lines: 0,
            id: tail.id,
            checkpoint: Vec::new(),
            #[cfg(unix)]
            file: None,
        };
        let mut out = Vec::new();
        drain_appended(
            &missing,
            &empty_filter(),
            Format::Json,
            &mut kept,
            &mut out,
            || false,
        )
        .unwrap();
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
            open_failure_reported: false,
            pos: sample_lines().len() as u64,
            lines: 5,
            id: None,
            checkpoint: Vec::new(),
            #[cfg(unix)]
            file: None,
        };
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, r#"{{"id":"5","kind":"http"}}"#).unwrap();

        let mut out = Vec::new();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();

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
            || false,
        )
        .unwrap();

        // The replacement is larger than the saved offset: only the identity gives it away.
        let replacement = sample_lines().replace("/x/", "/replaced/").repeat(2);
        assert!(replacement.len() as u64 > tail.pos);
        replace_by_rename(&path, &replacement);

        let mut out = Vec::new();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
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
            || false,
        )
        .unwrap();

        let replacement = sample_lines().replace("/x/", "/y/");
        assert_eq!(replacement.len() as u64, tail.pos);
        replace_by_rename(&path, &replacement);

        let mut out = Vec::new();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.is_empty(), "text was: {text}");
        assert_eq!(tail.pos, replacement.len() as u64);
        assert_follows_next_append(&path, &mut tail);
    }

    #[cfg(unix)]
    #[test]
    fn follow_pins_the_inode_across_two_replacements_between_polls() {
        use std::os::unix::fs::MetadataExt;

        for after_backlog in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("log.jsonl");
            std::fs::write(&path, sample_lines()).unwrap();
            let mut out = Vec::new();
            let mut tail = if after_backlog {
                let mut sink = BacklogSink {
                    format: Format::Json,
                    limit: Some(0),
                    ring: VecDeque::new(),
                    out: &mut out,
                };
                scan_files(&path, &empty_filter(), true, false, &mut sink, || false)
                    .unwrap()
                    .unwrap()
            } else {
                let mut tail = tail_at_start();
                drain_appended(
                    &path,
                    &empty_filter(),
                    Format::Json,
                    &mut tail,
                    &mut out,
                    || false,
                )
                .unwrap();
                tail
            };
            let original_id = tail.id;
            assert!(original_id.is_some());
            assert_eq!(file_id(tail.file.as_ref().unwrap()), original_id);

            out.clear();
            if !after_backlog {
                // An idle poll must retain a handle too, not just polls that read bytes.
                drain_appended(
                    &path,
                    &empty_filter(),
                    Format::Json,
                    &mut tail,
                    &mut out,
                    || false,
                )
                .unwrap();
                assert!(out.is_empty());
            }
            replace_by_rename(&path, &sample_lines().replace("/x/", "/first/"));
            let held = tail.file.as_ref().unwrap();
            assert_eq!(file_id(held), original_id);
            assert_eq!(held.metadata().unwrap().nlink(), 0);
            // The unlinked inode is still allocated to `held`, so it cannot be
            // reused for this second replacement. No poll occurs between renames.
            let replacement = sample_lines().replace("/x/", "/second/").repeat(2);
            replace_by_rename(&path, &replacement);
            let final_id = file_id(&File::open(&path).unwrap());
            assert_ne!(final_id, original_id);
            assert!(replacement.len() as u64 > tail.pos);
            drain_appended(
                &path,
                &empty_filter(),
                Format::Json,
                &mut tail,
                &mut out,
                || false,
            )
            .unwrap();
            assert!(out.is_empty(), "replacement contents must not replay");
            assert_eq!(tail.pos, replacement.len() as u64);
            assert_eq!(tail.id, final_id);
            assert_eq!(file_id(tail.file.as_ref().unwrap()), final_id);
            assert_follows_next_append(&path, &mut tail);
        }
    }

    #[cfg(unix)]
    #[test]
    fn drain_appended_pins_the_unlinked_inode_while_the_path_is_absent() {
        use std::os::unix::fs::MetadataExt;

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
            || false,
        )
        .unwrap();
        let original_id = tail.id;
        let original_pos = tail.pos;
        std::fs::remove_file(&path).unwrap();
        let mut out = Vec::new();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
        assert!(out.is_empty());
        assert_eq!(tail.pos, original_pos);
        let held = tail.file.as_ref().unwrap();
        assert_eq!(file_id(held), original_id);
        assert_eq!(held.metadata().unwrap().nlink(), 0);

        std::fs::write(&path, sample_lines().repeat(2)).unwrap();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
        assert!(out.is_empty());
        assert_ne!(tail.id, original_id);
        assert_eq!(tail.pos, sample_lines().len() as u64 * 2);
        assert_follows_next_append(&path, &mut tail);
    }

    fn assert_follows_next_append(path: &Path, tail: &mut Tail) {
        let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        writeln!(file, "{GOOD}").unwrap();
        let mut out = Vec::new();
        drain_appended(path, &empty_filter(), Format::Json, tail, &mut out, || {
            false
        })
        .unwrap();
        assert_eq!(out, format!("{GOOD}\n").as_bytes());
        out.clear();
        drain_appended(path, &empty_filter(), Format::Json, tail, &mut out, || {
            false
        })
        .unwrap();
        assert!(out.is_empty(), "the next append is printed exactly once");
    }

    #[test]
    fn follow_detects_same_inode_rewrites_at_equal_and_larger_lengths() {
        for larger in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("log.jsonl");
            let initial = sample_lines();
            std::fs::write(&path, &initial).unwrap();
            let mut out = Vec::new();
            let mut sink = BacklogSink {
                format: Format::Json,
                limit: None,
                ring: VecDeque::new(),
                out: &mut out,
            };
            let mut tail = scan_files(&path, &empty_filter(), true, false, &mut sink, || false)
                .unwrap()
                .unwrap();
            assert_eq!(out, initial.as_bytes());
            let id = tail.id;
            let checkpoint = tail.checkpoint.clone();
            let rewritten = initial
                .replace("/x/", "/y/")
                .repeat(if larger { 2 } else { 1 });
            assert!(rewritten.len() as u64 >= tail.pos);
            // fs::write truncates the existing inode and regrows it before one drain.
            std::fs::write(&path, &rewritten).unwrap();
            assert_eq!(file_id(&File::open(&path).unwrap()), id);
            out.clear();
            drain_appended(
                &path,
                &empty_filter(),
                Format::Json,
                &mut tail,
                &mut out,
                || false,
            )
            .unwrap();
            assert!(out.is_empty(), "rewritten records must be skipped");
            assert_eq!(tail.pos, rewritten.len() as u64);
            assert_eq!(tail.lines, if larger { 10 } else { 5 });
            assert_ne!(tail.checkpoint, checkpoint);
            assert_follows_next_append(&path, &mut tail);
            // Checkpoint refresh after an append must also detect a later rewrite.
            let later = std::fs::read_to_string(&path)
                .unwrap()
                .replace("http", "xxxx");
            std::fs::write(&path, &later).unwrap();
            drain_appended(
                &path,
                &empty_filter(),
                Format::Json,
                &mut tail,
                &mut out,
                || false,
            )
            .unwrap();
            assert!(out.is_empty());
            assert_eq!(
                tail.checkpoint,
                read_checkpoint(&File::open(&path).unwrap(), tail.pos).unwrap()
            );
            assert_follows_next_append(&path, &mut tail);
        }
    }

    #[test]
    fn follow_checkpoint_preserves_partial_appends_and_idle_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, format!("{GOOD}\n")).unwrap();
        let mut tail = Tail::default();
        let mut out = Vec::new();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
        let pos = tail.pos;
        let checkpoint = tail.checkpoint.clone();
        out.clear();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        write!(file, "{GOOD}").unwrap();
        for _ in 0..3 {
            drain_appended(
                &path,
                &empty_filter(),
                Format::Json,
                &mut tail,
                &mut out,
                || false,
            )
            .unwrap();
            assert!(out.is_empty());
            assert_eq!(tail.pos, pos);
            assert_eq!(tail.checkpoint, checkpoint);
        }
        writeln!(file).unwrap();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
        assert_eq!(out, format!("{GOOD}\n").as_bytes());
        assert_follows_next_append(&path, &mut tail);
    }

    #[test]
    fn checkpoint_reads_only_bounded_raw_bytes_before_the_offset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        let bytes = vec![0xff; TAIL_CHECKPOINT_BYTES * 4];
        std::fs::write(&path, &bytes).unwrap();
        let file = File::open(&path).unwrap();
        assert!(read_checkpoint(&file, 0).unwrap().is_empty());
        assert_eq!(read_checkpoint(&file, 7).unwrap(), bytes[..7]);
        assert_eq!(
            read_checkpoint(&file, bytes.len() as u64).unwrap(),
            bytes[..TAIL_CHECKPOINT_BYTES]
        );
        // A concurrent shrink yields fewer bytes rather than an unexpected-EOF error.
        std::fs::write(&path, []).unwrap();
        assert!(read_checkpoint(&file, 7).unwrap().is_empty());
    }

    #[test]
    fn unchanged_checkpoint_cannot_reveal_an_earlier_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        let initial = sample_lines();
        std::fs::write(&path, &initial).unwrap();
        let mut tail = Tail::default();
        let mut out = Vec::new();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
        let checkpoint = tail.checkpoint.clone();
        let rewritten = initial.replacen("/x/", "/y/", 1);
        std::fs::write(&path, &rewritten).unwrap();
        out.clear();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
        assert!(out.is_empty());
        assert_eq!(tail.pos, initial.len() as u64);
        assert_eq!(tail.checkpoint, checkpoint);
        assert_follows_next_append(&path, &mut tail);
    }

    #[test]
    fn drain_appended_reads_a_file_created_after_a_missing_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        let mut tail = tail_at_start();
        let mut out = Vec::new();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
        std::fs::write(&path, sample_lines()).unwrap();
        drain_appended(
            &path,
            &empty_filter(),
            Format::Json,
            &mut tail,
            &mut out,
            || false,
        )
        .unwrap();
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
                        open_failure_reported: false,
                        pos: 0,
                        lines: 0,
                        id: file_id(&file),
                        checkpoint: Vec::new(),
                        #[cfg(unix)]
                        file: None,
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
                    let Backlog::Complete { pos, lines, .. } = result else {
                        panic!("backlog should complete");
                    };
                    tail.pos = pos;
                    tail.lines = lines;
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
                    drain_appended(
                        &path,
                        &empty_filter(),
                        Format::Json,
                        &mut tail,
                        &mut out,
                        || false,
                    )
                    .unwrap();
                    assert!(out.is_empty(), "replacement must never replay records");
                    assert_eq!(tail.pos, replacement.len() as u64);
                    assert_follows_next_append(&path, &mut tail);
                }
            }
        }
    }
}
