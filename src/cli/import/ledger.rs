//! The Drive half of `gwi import`: copy omni-dev's lease ledger.
//!
//! gwi reads `<state_dir>/gwi/lease-ledger.jsonl` (ADR-0001 §2) and never omni-dev's
//! `<state_dir>/omni-dev/lease-ledger.jsonl`, so without this a user switching tools
//! loses the only record of which lease tokens are valid and which backups they point
//! at. The rules mirror the settings import (see [`super`]):
//!
//! - It **copies**. The source ledger is only read: it is never rewritten, and nothing
//!   is created beside it (no directory, no lock file).
//! - It merges one lease (one token) at a time. A token gwi lacks is added; an identical
//!   row is unchanged, so a second run changes nothing; a *different* row for the same
//!   token (gwi has since written under it) is a conflict and is left alone unless
//!   `--force` is given. A lease gwi has *released* is never re-activated, with or
//!   without `--force`: omni-dev's copy of it is stale, and bringing it back would make a
//!   released token valid again.
//! - **A release in omni-dev propagates.** When omni-dev has released a lease that gwi's
//!   row is unreleased, and the token and file id match, gwi's row takes omni-dev's
//!   `released_at` (and `superseded_by`) and is reported as `released`. This is the mirror
//!   of the rule above: propagating a release only ends authority, so it is
//!   always on. All other fields of gwi's row are kept, even if the copies have diverged.
//!   A lease both tools released for the same file is unchanged. Live-row divergence
//!   and a token naming different files remain conflicts unless `--force` is given.
//!   Even freshness-only divergence stays a conflict: replacing `version` can restore
//!   write authority to a stale token. No ordering of Drive versions is assumed.
//! - **Every row is copied, expired and released ones included.** The ledger keeps them
//!   on purpose: `drive lease restore` finds a backup by token, and a restore is almost
//!   always wanted after the expiry window (ADR-0080 §4).
//! - **A live lease is carried over unchanged.** It stays valid in gwi until its absolute
//!   expiry, and the file's `version` is still checked on every write, so a write made
//!   through the other tool is refused rather than overwritten. The two ledgers are
//!   copies, not shared: a release in omni-dev reaches gwi only on the next import, which
//!   is why the report warns about the live ones it carried.
//! - Both ledgers' advisory locks are held while copying (omni-dev's only if its lock
//!   file already exists, because taking a lock creates the file and the source must not
//!   be touched). A dry run takes no lock and writes nothing.
//! - **A field gwi does not know is copied too.** A row from a newer omni-dev keeps its
//!   unknown fields, including those inside `backup`, through the import and every later
//!   rewrite of the ledger (`LeaseRecord::extra` and `LeaseBackup` flatten them),
//!   subject to the [private number key limitation](../../../docs/lease-ledger-json.md):
//!   some valid unknown JSON objects become numbers or prevent the ledger loading.
//! - No audit history is copied (ADR-0001 §3).
//! - The report names tokens (identifiers, not credentials: ADR-0080 §2) and never a
//!   backup path, hash or file id.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};

use super::Status;
use crate::drive::lease::ledger::{
    default_lock_wait_timeout, lock_path_for, LeaseLedger, LeaseRecord, LedgerLock,
};
use crate::utils::fs::{try_lock_or_busy, FileLock};

/// How often a busy source lock is retried.
const SOURCE_LOCK_POLL: Duration = Duration::from_millis(50);

/// omni-dev's ledger: `<state_dir>/omni-dev/lease-ledger.jsonl`, beside gwi's own
/// `<state_dir>/gwi/` and resolved the same way (`state_dir`, falling back to `data_dir`).
pub(super) fn default_source_ledger() -> Result<PathBuf> {
    let base = dirs::state_dir()
        .or_else(dirs::data_dir)
        .context("could not resolve the state/data directory for omni-dev's lease ledger")?;
    Ok(base.join("omni-dev").join("lease-ledger.jsonl"))
}

/// What the merge did to one lease, and whether the lease was live when it arrived.
#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    token: String,
    status: Status,
    /// The row was copied in (added or overwritten) and can still authorise a write.
    carried_live: bool,
    /// Why the row was left as it is, when that is not obvious from the status.
    note: Option<&'static str>,
}

/// Returns whether unreleased rows differ only in their write freshness metadata.
/// Compares all other fields, including unknown fields, without ordering versions.
fn differs_only_in_freshness(current: &LeaseRecord, source: &LeaseRecord) -> bool {
    if current.released_at.is_some() || source.released_at.is_some() || current == source {
        return false;
    }
    let mut normalized = source.clone();
    normalized.version.clone_from(&current.version);
    normalized.modified_time.clone_from(&current.modified_time);
    current == &normalized
}

/// Merges `source` into `target`, propagating releases for matching lease identities.
/// Other differing rows are replaced only with `force`; released leases are never revived.
fn merge(
    target: &mut LeaseLedger,
    source: &LeaseLedger,
    force: bool,
    now: DateTime<Utc>,
) -> Vec<Outcome> {
    let mut outcomes = Vec::new();
    for record in source.iter() {
        let mut note = None;
        let status = match target.get(&record.token) {
            None => Status::Added,
            Some(current) if current == record => Status::Unchanged,
            // gwi ended this lease; omni-dev's row still shows it live. Never revive it.
            Some(current) if current.released_at.is_some() && record.released_at.is_none() => {
                note = Some("gwi released it; not re-activated");
                Status::Unchanged
            }
            // End authority for the same lease identity, preserving local metadata.
            // Once gwi has released it too, subsequent imports are a no-op.
            Some(current) if record.released_at.is_some() && current.file_id == record.file_id => {
                if current.released_at.is_some() {
                    note = Some("already released in gwi");
                    Status::Unchanged
                } else {
                    note = Some("released in omni-dev");
                    Status::Released
                }
            }
            Some(_) if force => Status::Overwritten,
            Some(current) => {
                if differs_only_in_freshness(current, record) {
                    note = Some(
                        "unreleased copies differ only in freshness; omni-dev may have written \
                         under this lease; gwi kept its row",
                    );
                }
                Status::Conflict
            }
        };
        let copied = matches!(status, Status::Added | Status::Overwritten);
        if copied {
            target.insert(record.clone());
        } else if status == Status::Released {
            // End authority only: keep every other field of gwi's row as it is.
            if let Some(mut row) = target.get(&record.token).cloned() {
                row.released_at = record.released_at;
                row.superseded_by.clone_from(&record.superseded_by);
                target.insert(row);
            }
        }
        outcomes.push(Outcome {
            token: record.token.clone(),
            status,
            carried_live: copied && record.is_live(now),
            note,
        });
    }
    outcomes
}

/// Takes omni-dev's ledger lock if it has one, waiting up to `max_wait` for a holder.
///
/// `None` when the lock file does not exist: omni-dev creates it before it ever writes
/// the ledger, so its absence means nothing is writing, and creating it here would
/// modify the source. `None` too, with a warning, when the file exists but cannot be
/// opened for locking (a read-only copy, another owner): the ledger is replaced by
/// rename, so reading it without the lock still sees a whole file.
fn lock_source(source: &Path, max_wait: Duration) -> Result<(Option<FileLock>, Option<String>)> {
    let lock_path = lock_path_for(source);
    if !lock_path.exists() {
        return Ok((None, None));
    }
    let start = Instant::now();
    loop {
        match try_lock_or_busy(&lock_path, "omni-dev's lease lock file") {
            Ok(Some(lock)) => return Ok((Some(lock), None)),
            Ok(None) => {}
            Err(err) => {
                let warning = format!(
                    "could not lock omni-dev's ledger ({err:#}); reading it without the lock"
                );
                return Ok((None, Some(warning)));
            }
        }
        if start.elapsed() >= max_wait {
            return Err(anyhow!(
                "timed out after {max_wait:?} waiting for omni-dev to release its lease ledger \
                 lock ({} is locked); retry once any running `omni-dev drive lease` command \
                 has finished",
                lock_path.display()
            ));
        }
        std::thread::sleep(SOURCE_LOCK_POLL);
    }
}

/// Imports the lease ledger at `source` into `target`, writing the report to `out`.
///
/// A missing source is not an error: most omni-dev users never leased a Drive write.
/// Returns an error when the source is unusable, or when conflicts were left unresolved
/// (after importing everything that could be imported safely). A dry run reports its
/// conflicts but does not fail on them: they are part of the preview.
pub(super) fn run_ledger_import(
    source: &Path,
    target: &Path,
    dry_run: bool,
    force: bool,
    out: &mut impl Write,
) -> Result<usize> {
    run_ledger_import_waiting(
        source,
        target,
        dry_run,
        force,
        default_lock_wait_timeout(),
        out,
    )
}

/// [`run_ledger_import`] with an explicit lock wait budget, the seam tests use to
/// exercise a busy lock in milliseconds.
fn run_ledger_import_waiting(
    source: &Path,
    target: &Path,
    dry_run: bool,
    force: bool,
    lock_wait: Duration,
    out: &mut impl Write,
) -> Result<usize> {
    match std::fs::symlink_metadata(source) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            writeln!(
                out,
                "No omni-dev lease ledger at {}; no leases to import (pass --source-ledger to \
                 point at one).",
                source.display()
            )?;
            return Ok(0);
        }
        Err(e) => return Err(e).with_context(|| format!("Failed to inspect {}", source.display())),
        Ok(_) if !source.is_file() => {
            return Err(anyhow!(
                "{} is not a lease ledger file; pass --source-ledger with the path of \
                 lease-ledger.jsonl",
                source.display()
            ))
        }
        Ok(_) => {}
    }
    writeln!(
        out,
        "Importing leases from {} into {}",
        source.display(),
        target.display()
    )?;

    // Source lock first, then gwi's: every import takes them in this order, and omni-dev
    // never takes gwi's, so two imports cannot deadlock.
    let (_source_lock, lock_warning) = if dry_run {
        (None, None)
    } else {
        lock_source(source, lock_wait)?
    };
    if let Some(warning) = lock_warning {
        writeln!(out, "warning: {warning}")?;
    }
    let source_ledger = LeaseLedger::load(source)?;
    if source_ledger.iter().next().is_none() {
        writeln!(out, "Nothing to import: the source ledger holds no leases.")?;
        return Ok(0);
    }
    let _target_lock = if dry_run {
        None
    } else {
        Some(LedgerLock::acquire_waiting_blocking_with_timeout(
            target, lock_wait,
        )?)
    };
    let mut target_ledger = LeaseLedger::load(target)?;
    let outcomes = merge(&mut target_ledger, &source_ledger, force, Utc::now());

    let count = |status: Status| outcomes.iter().filter(|o| o.status == status).count();
    for outcome in &outcomes {
        let note = match (outcome.status, outcome.note) {
            (_, Some(note)) => format!("  ({note})"),
            (Status::Conflict, None) => "  (gwi has a different row; kept unchanged)".to_string(),
            _ => String::new(),
        };
        writeln!(
            out,
            "  {:<11} lease {}{note}",
            outcome.status.label(),
            outcome.token
        )?;
    }
    let live = outcomes.iter().filter(|o| o.carried_live).count();
    if live > 0 {
        writeln!(
            out,
            "warning: {live} live lease(s) were carried over and stay valid in gwi until they \
             expire; the two ledgers are copies, so releasing a lease in omni-dev ends gwi's \
             copy only on a later `gwi import` (or with `gwi drive lease release`)"
        )?;
    }
    let conflicts = count(Status::Conflict);
    if conflicts > 0 {
        writeln!(
            out,
            "For conflicting unreleased leases, --force replaces the entire row, including \
             version, modified_time, expiry, file binding, backup/restore metadata and unknown \
             fields; replacing version may restore write authority to a stale token. \
             Recommended recovery after switching tools: release the old lease in omni-dev \
             and run `gwi import` to propagate the release when token and file ID match \
             (or use `gwi drive lease release`), then acquire a fresh authorized lease with \
             `gwi drive lease acquire`."
        )?;
    }
    writeln!(
        out,
        "{}{} added, {} overwritten, {} released, {} unchanged, {} conflict(s).",
        if dry_run {
            "Dry run, nothing written: "
        } else {
            ""
        },
        count(Status::Added),
        count(Status::Overwritten),
        count(Status::Released),
        count(Status::Unchanged),
        conflicts,
    )?;

    if !dry_run && count(Status::Added) + count(Status::Overwritten) + count(Status::Released) > 0 {
        target_ledger.save(target)?;
    }
    if conflicts > 0 && !dry_run {
        return Err(anyhow!(
            "{conflicts} lease(s) were not imported because gwi already has a different row for \
             the token; rows were kept unchanged. --force replaces entire conflicting rows and may \
             restore write authority; release the old lease and acquire a fresh authorized lease \
             instead when switching tools after a write"
        ));
    }
    Ok(conflicts)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::drive::lease::ledger::LeaseRecord;
    use chrono::Duration as ChronoDuration;

    /// A backup path that must never appear in a report.
    const BACKUP_PATH: &str = "/Users/someone/backups/secret-looking-name.bin";

    fn record(token: &str, expires_in_minutes: i64) -> LeaseRecord {
        let json = serde_json::json!({
            "token": token,
            "file_id": "file-id-1",
            "version": "3",
            "modified_time": "2026-09-11T00:00:00Z",
            "backup": {"kind": "bytes", "path": BACKUP_PATH, "sha256": "abc123", "size": 42},
            "acquired_at": Utc::now() - ChronoDuration::minutes(1),
            "expires_at": Utc::now() + ChronoDuration::minutes(expires_in_minutes),
        });
        serde_json::from_value(json).unwrap()
    }

    fn ledger_of(records: Vec<LeaseRecord>) -> LeaseLedger {
        let mut ledger = LeaseLedger::default();
        for record in records {
            ledger.insert(record);
        }
        ledger
    }

    /// A source ledger holding a live, an expired and a released lease, and a target path.
    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("omni-dev").join("lease-ledger.jsonl");
        let target = dir.path().join("gwi").join("lease-ledger.jsonl");
        let mut released = record("released", 10);
        released.released_at = Some(Utc::now());
        ledger_of(vec![record("live", 10), record("expired", -10), released])
            .save(&source)
            .unwrap();
        (dir, source, target)
    }

    fn import(source: &Path, target: &Path, dry_run: bool, force: bool) -> (Result<usize>, String) {
        let mut out = Vec::new();
        let result = run_ledger_import(source, target, dry_run, force, &mut out);
        (result, String::from_utf8(out).unwrap())
    }

    fn tokens(path: &Path) -> Vec<String> {
        LeaseLedger::load(path)
            .unwrap()
            .iter()
            .map(|r| r.token.clone())
            .collect()
    }

    #[test]
    fn every_row_is_copied_including_expired_and_released_ones() {
        let (_dir, source, target) = setup();

        let (result, report) = import(&source, &target, false, false);

        result.unwrap();
        assert_eq!(tokens(&target), ["expired", "live", "released"]);
        assert_eq!(
            LeaseLedger::load(&target).unwrap().get("live"),
            LeaseLedger::load(&source).unwrap().get("live"),
            "a row must arrive unchanged"
        );
        for token in ["expired", "live", "released"] {
            assert!(
                report.contains(&format!("added       lease {token}")),
                "{report}"
            );
        }
        assert!(report.contains("3 added, 0 overwritten, 0 released, 0 unchanged, 0 conflict(s)"));
    }

    #[test]
    fn a_live_lease_is_carried_over_with_a_warning() {
        let (_dir, source, target) = setup();

        let (_, report) = import(&source, &target, false, false);

        assert!(report.contains("warning: 1 live lease(s)"), "{report}");
        let live = LeaseLedger::load(&target).unwrap();
        assert!(live.get("live").unwrap().is_live(Utc::now()));
    }

    #[test]
    fn expired_and_released_rows_alone_raise_no_warning() {
        let (_dir, source, target) = setup();
        let mut ledger = LeaseLedger::load(&source).unwrap();
        ledger.remove("live");
        ledger.save(&source).unwrap();

        let (_, report) = import(&source, &target, false, false);

        assert!(!report.contains("warning"), "{report}");
    }

    #[test]
    fn a_second_run_changes_nothing() {
        let (_dir, source, target) = setup();
        import(&source, &target, false, false).0.unwrap();
        let after_first = std::fs::read(&target).unwrap();

        let (result, report) = import(&source, &target, false, false);

        result.unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), after_first);
        assert!(report.contains("0 added, 0 overwritten, 0 released, 3 unchanged, 0 conflict(s)"));
        assert!(!report.contains("warning"), "{report}");
    }

    /// A source row with a field this build does not know, as a newer omni-dev would write.
    fn newer_row() -> LeaseRecord {
        let mut row = serde_json::to_value(record("newer", 10)).unwrap();
        row["field_from_a_newer_omni_dev"] = serde_json::json!({"mode": "strict", "n": [1, 2]});
        serde_json::from_value(row).unwrap()
    }

    #[test]
    fn a_field_from_a_newer_omni_dev_survives_the_import_and_a_later_rewrite() {
        let (_dir, source, target) = setup();
        ledger_of(vec![newer_row()]).save(&source).unwrap();

        let (result, _) = import(&source, &target, false, false);
        result.unwrap();

        let want = serde_json::json!({"mode": "strict", "n": [1, 2]});
        let on_disk = std::fs::read_to_string(&target).unwrap();
        let row: serde_json::Value = serde_json::from_str(on_disk.trim()).unwrap();
        assert_eq!(row["field_from_a_newer_omni_dev"], want);

        // A later `gwi drive lease` change reloads and rewrites the whole ledger.
        let lock = LedgerLock::acquire(&target).unwrap();
        LeaseLedger::mutate(&lock, &target, |ledger| {
            ledger.record_write("newer", "4".into(), None);
        })
        .unwrap();
        let rewritten = LeaseLedger::load(&target).unwrap();
        let rec = rewritten.get("newer").unwrap();
        assert_eq!(rec.version, "4");
        assert_eq!(rec.extra["field_from_a_newer_omni_dev"], want);
    }

    #[test]
    fn unknown_backup_fields_survive_import_rewrite_and_conflict_resolution() {
        let backups: Vec<serde_json::Value> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/lease-backups-unknown-fields.json"
        ))
        .unwrap();
        for backup in backups {
            let (_dir, source, target) = setup();
            let mut row = serde_json::to_value(record("newer", 10)).unwrap();
            row["backup"] = backup.clone();
            std::fs::write(&source, format!("{row}\n")).unwrap();
            import(&source, &target, false, false).0.unwrap();
            let saved: serde_json::Value =
                serde_json::from_str(std::fs::read_to_string(&target).unwrap().trim()).unwrap();
            assert_eq!(saved, row);

            let lock = LedgerLock::acquire(&target).unwrap();
            LeaseLedger::mutate(&lock, &target, |ledger| {
                ledger.record_write("newer", "4".into(), None);
            })
            .unwrap();
            drop(lock);
            let mut saved: serde_json::Value =
                serde_json::from_str(std::fs::read_to_string(&target).unwrap().trim()).unwrap();
            assert_eq!(saved["backup"], backup);
            assert_eq!(saved["version"], "4");

            // Keep every known field equal; only unknown backup metadata differs.
            saved["backup"]["metadata"] = serde_json::json!({"changed": true});
            std::fs::write(&source, format!("{saved}\n")).unwrap();
            let before = std::fs::read(&target).unwrap();
            let (result, report) = import(&source, &target, false, false);
            assert!(result.is_err(), "{report}");
            assert!(report.contains("conflict    lease newer"), "{report}");
            assert_eq!(std::fs::read(&target).unwrap(), before);

            import(&source, &target, false, true).0.unwrap();
            let forced: serde_json::Value =
                serde_json::from_str(std::fs::read_to_string(&target).unwrap().trim()).unwrap();
            assert_eq!(forced, saved);
        }
    }

    #[test]
    fn rows_differing_only_in_an_unknown_field_are_a_conflict() {
        let (_dir, source, target) = setup();
        let gwi_row = record("newer", 10);
        let mut source_row = gwi_row.clone();
        source_row
            .extra
            .insert("field_from_a_newer_omni_dev".into(), serde_json::json!(1));
        ledger_of(vec![source_row]).save(&source).unwrap();
        ledger_of(vec![gwi_row]).save(&target).unwrap();

        let (result, report) = import(&source, &target, false, false);

        assert!(result.is_err(), "{report}");
        assert!(report.contains("conflict    lease newer"), "{report}");
        assert!(LeaseLedger::load(&target)
            .unwrap()
            .get("newer")
            .unwrap()
            .extra
            .is_empty());

        let (result, _) = import(&source, &target, false, true);
        result.unwrap();
        assert!(!LeaseLedger::load(&target)
            .unwrap()
            .get("newer")
            .unwrap()
            .extra
            .is_empty());
    }

    #[test]
    fn a_source_write_without_release_keeps_gwi_freshness_and_authority_unchanged() {
        let (_dir, source, target) = setup();
        import(&source, &target, false, false).0.unwrap();
        let original = LeaseLedger::load(&target).unwrap();
        let before = std::fs::read(&target).unwrap();
        let mut written = LeaseLedger::load(&source).unwrap();
        written.record_write("live", "4".into(), Some("2026-10-09T00:00:00Z".into()));
        written.save(&source).unwrap();
        let source_before = std::fs::read(&source).unwrap();

        for dry_run in [false, true] {
            let (result, report) = import(&source, &target, dry_run, false);
            if dry_run {
                assert_eq!(result.unwrap(), 1);
            } else {
                let error = result.unwrap_err().to_string();
                assert!(error.contains("release the old lease"), "{error}");
            }
            assert!(report.contains("conflict    lease live"), "{report}");
            assert!(
                report.contains("unreleased copies differ only in freshness"),
                "{report}"
            );
            assert!(report.contains("omni-dev may have written"), "{report}");
            assert!(
                report.contains("--force replaces the entire row"),
                "{report}"
            );
            assert!(report.contains("gwi drive lease acquire"), "{report}");
            assert!(!report.contains(BACKUP_PATH), "{report}");
            assert_eq!(std::fs::read(&target).unwrap(), before);
            assert_eq!(
                LeaseLedger::load(&target).unwrap().get("live"),
                original.get("live")
            );
            assert_eq!(std::fs::read(&source).unwrap(), source_before);
        }
        let (result, report) = import(&source, &target, false, true);
        result.unwrap();
        assert!(report.contains("overwritten lease live"), "{report}");
        assert_eq!(
            LeaseLedger::load(&target).unwrap().get("live"),
            written.get("live")
        );
    }

    #[test]
    fn freshness_diagnostics_require_equality_of_every_other_field() {
        let current = record("live", 10);
        assert!(!differs_only_in_freshness(&current, &current));
        // Neither direction nor numeric ordering gives freshness authority.
        for version in ["2", "4", "opaque-version"] {
            let mut source = current.clone();
            source.version = version.into();
            assert!(differs_only_in_freshness(&current, &source));
            assert!(differs_only_in_freshness(&source, &current));
        }
        let mut source = current.clone();
        source.modified_time = None;
        assert!(differs_only_in_freshness(&current, &source));
        source.version = "4".into();
        let value = serde_json::to_value(&source).unwrap();
        for (field, different) in [
            ("token", serde_json::json!("different-token")),
            ("file_id", serde_json::json!("different-file")),
            ("expires_at", serde_json::json!("2999-01-01T00:00:00Z")),
            ("acquired_at", serde_json::json!("2020-01-01T00:00:00Z")),
            (
                "backup",
                serde_json::json!({"kind": "drive_copy", "file_id": "other-backup"}),
            ),
            ("restored_at", serde_json::json!("2026-10-09T00:00:00Z")),
            ("released_at", serde_json::json!("2026-10-09T00:00:00Z")),
            ("superseded_by", serde_json::json!("other-token")),
            ("unknown_field", serde_json::json!({"n": 1})),
        ] {
            let mut changed = value.clone();
            changed[field] = different;
            let changed: LeaseRecord = serde_json::from_value(changed).unwrap();
            assert!(!differs_only_in_freshness(&current, &changed), "{field}");
            let outcomes = merge(
                &mut ledger_of(vec![current.clone()]),
                &ledger_of(vec![changed]),
                false,
                Utc::now(),
            );
            assert!(
                outcomes[0]
                    .note
                    .is_none_or(|note| !note.contains("only in freshness")),
                "{field}"
            );
        }
    }

    #[test]
    fn a_differing_row_is_a_conflict_but_the_rest_still_import() {
        let (_dir, source, target) = setup();
        let mut changed = record("live", 10);
        changed.version = "9".to_string();
        ledger_of(vec![changed.clone()]).save(&target).unwrap();

        let (result, report) = import(&source, &target, false, false);

        let err = result.unwrap_err().to_string();
        assert!(err.contains("--force"), "{err}");
        assert!(report.contains("conflict    lease live"), "{report}");
        let merged = LeaseLedger::load(&target).unwrap();
        assert_eq!(merged.get("live"), Some(&changed), "gwi's row must survive");
        assert!(merged.get("expired").is_some() && merged.get("released").is_some());
    }

    #[test]
    fn a_dry_run_reports_a_conflict_without_failing() {
        let (_dir, source, target) = setup();
        let mut changed = record("live", 10);
        changed.version = "9".to_string();
        ledger_of(vec![changed]).save(&target).unwrap();
        let before = std::fs::read(&target).unwrap();

        let (result, report) = import(&source, &target, true, false);

        assert_eq!(result.unwrap(), 1);
        assert!(report.contains("conflict    lease live"), "{report}");
        assert!(report.contains("1 conflict(s)."), "{report}");
        assert_eq!(std::fs::read(&target).unwrap(), before);
        assert!(import(&source, &target, false, false).0.is_err());
    }

    #[test]
    fn force_overwrites_a_conflicting_row() {
        let (_dir, source, target) = setup();
        let mut changed = record("live", 10);
        changed.version = "9".to_string();
        ledger_of(vec![changed]).save(&target).unwrap();

        let (result, report) = import(&source, &target, false, true);

        result.unwrap();
        assert!(report.contains("overwritten lease live"), "{report}");
        assert_eq!(
            LeaseLedger::load(&target)
                .unwrap()
                .get("live")
                .unwrap()
                .version,
            "3"
        );
    }

    #[test]
    fn unrelated_gwi_leases_survive() {
        let (_dir, source, target) = setup();
        ledger_of(vec![record("gwi-own", 10)])
            .save(&target)
            .unwrap();

        import(&source, &target, false, false).0.unwrap();

        assert_eq!(tokens(&target), ["expired", "gwi-own", "live", "released"]);
    }

    #[test]
    fn a_dry_run_reports_but_writes_nothing() {
        let (dir, source, target) = setup();

        let (result, report) = import(&source, &target, true, false);

        result.unwrap();
        assert!(
            report.contains("Dry run, nothing written: 3 added"),
            "{report}"
        );
        assert!(
            !dir.path().join("gwi").exists(),
            "no directory, file or lock"
        );
    }

    #[test]
    fn the_source_is_never_modified_and_gets_no_lock_file() {
        let (dir, source, target) = setup();
        let before = std::fs::read(&source).unwrap();
        let mtime = std::fs::metadata(&source).unwrap().modified().unwrap();

        import(&source, &target, false, false).0.unwrap();
        import(&source, &target, true, false).0.unwrap();

        assert_eq!(std::fs::read(&source).unwrap(), before);
        assert_eq!(
            std::fs::metadata(&source).unwrap().modified().unwrap(),
            mtime
        );
        let entries: Vec<_> = std::fs::read_dir(dir.path().join("omni-dev"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            entries,
            ["lease-ledger.jsonl"],
            "nothing may appear beside the source"
        );
    }

    #[test]
    fn an_existing_source_lock_is_taken_and_a_held_one_is_waited_for() {
        let (_dir, source, target) = setup();
        let held = try_lock_or_busy(&lock_path_for(&source), "test lock").unwrap();
        assert!(held.is_some(), "the test must hold omni-dev's lock");

        let mut out = Vec::new();
        let err = run_ledger_import_waiting(
            &source,
            &target,
            false,
            false,
            Duration::from_millis(120),
            &mut out,
        )
        .unwrap_err()
        .to_string();

        assert!(
            err.contains("timed out") && err.contains("omni-dev"),
            "{err}"
        );
        assert!(
            !target.exists(),
            "nothing is written while the source is locked"
        );
        drop(held);
        import(&source, &target, false, false).0.unwrap();
        assert_eq!(tokens(&target).len(), 3);
    }

    #[test]
    fn a_held_gwi_lock_blocks_the_write_until_the_budget_runs_out() {
        let (_dir, source, target) = setup();
        let _held = LedgerLock::acquire(&target).unwrap();

        let mut out = Vec::new();
        let err = run_ledger_import_waiting(
            &source,
            &target,
            false,
            false,
            Duration::from_millis(120),
            &mut out,
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("timed out"), "{err}");
        assert!(!target.exists());
    }

    #[test]
    fn a_lease_gwi_released_is_never_revived_even_with_force() {
        let (_dir, source, target) = setup();
        let mut released = record("live", 10);
        released.released_at = Some(Utc::now());
        ledger_of(vec![released.clone()]).save(&target).unwrap();

        for force in [false, true] {
            let (result, report) = import(&source, &target, false, force);

            result.unwrap();
            assert!(
                report.contains("gwi released it; not re-activated"),
                "{report}"
            );
            let kept = LeaseLedger::load(&target).unwrap();
            assert_eq!(kept.get("live"), Some(&released), "force={force}");
        }
    }

    /// Releases `token` in the source ledger, as `omni-dev drive lease release` would.
    fn release_in_source(source: &Path, token: &str, superseded_by: Option<&str>) {
        let mut ledger = LeaseLedger::load(source).unwrap();
        let mut row = ledger.get(token).unwrap().clone();
        row.released_at = Some(Utc::now());
        row.superseded_by = superseded_by.map(str::to_string);
        ledger.insert(row);
        ledger.save(source).unwrap();
    }

    #[test]
    fn a_release_in_omni_dev_after_an_import_is_released_in_gwi() {
        let (_dir, source, target) = setup();
        import(&source, &target, false, false).0.unwrap();
        release_in_source(&source, "live", None);

        let (result, report) = import(&source, &target, false, false);

        result.unwrap();
        let merged = LeaseLedger::load(&target).unwrap();
        let row = merged.get("live").unwrap();
        assert!(!row.is_live(Utc::now()));
        assert_eq!(
            row.released_at,
            LeaseLedger::load(&source)
                .unwrap()
                .get("live")
                .unwrap()
                .released_at
        );
        assert_eq!(
            merged.get("live"),
            LeaseLedger::load(&source).unwrap().get("live")
        );
        assert!(
            report.contains("released    lease live  (released in omni-dev)"),
            "{report}"
        );
        assert!(
            report.contains("0 added, 0 overwritten, 1 released, 2 unchanged, 0 conflict(s)"),
            "{report}"
        );
        assert!(!report.contains("warning"), "{report}");
    }

    #[test]
    fn a_second_run_after_a_release_changes_nothing() {
        let (_dir, source, target) = setup();
        import(&source, &target, false, false).0.unwrap();
        release_in_source(&source, "live", None);
        import(&source, &target, false, false).0.unwrap();
        let after_release = std::fs::read(&target).unwrap();

        let (result, report) = import(&source, &target, false, false);

        result.unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), after_release);
        assert!(
            report.contains("0 added, 0 overwritten, 0 released, 3 unchanged, 0 conflict(s)"),
            "{report}"
        );
    }

    #[test]
    fn a_release_carries_superseded_by_with_it() {
        let (_dir, source, target) = setup();
        import(&source, &target, false, false).0.unwrap();
        release_in_source(&source, "live", Some("replacement"));

        import(&source, &target, false, false).0.unwrap();

        let merged = LeaseLedger::load(&target).unwrap();
        assert_eq!(
            merged.get("live").unwrap().superseded_by.as_deref(),
            Some("replacement")
        );
    }

    #[test]
    fn a_release_of_diverged_copies_preserves_local_fields_and_is_idempotent() {
        for force in [false, true] {
            let (_dir, source, target) = setup();
            import(&source, &target, false, false).0.unwrap();
            let mut ledger = LeaseLedger::load(&source).unwrap();
            let mut row = ledger.get("live").unwrap().clone();
            row.version = "9".into();
            row.modified_time = Some("2026-10-09T00:00:00Z".into());
            row.backup = crate::drive::lease::ledger::LeaseBackup::DriveCopy {
                file_id: "other-backup".into(),
                extra: serde_json::Map::new(),
            };
            row.acquired_at -= ChronoDuration::minutes(1);
            row.expires_at += ChronoDuration::hours(1);
            row.restored_at = Some(Utc::now());
            row.restored_sheet_id = Some(42);
            row.extra
                .insert("new_field".into(), serde_json::json!(true));
            ledger.insert(row);
            ledger.save(&source).unwrap();
            release_in_source(&source, "live", Some("replacement"));
            let source_before = std::fs::read(&source).unwrap();
            let source_row = LeaseLedger::load(&source)
                .unwrap()
                .get("live")
                .unwrap()
                .clone();
            let mut expected = LeaseLedger::load(&target)
                .unwrap()
                .get("live")
                .unwrap()
                .clone();
            expected.released_at = source_row.released_at;
            expected.superseded_by = source_row.superseded_by;

            let before = std::fs::read(&target).unwrap();
            let (result, report) = import(&source, &target, true, force);
            result.unwrap();
            assert!(report.contains("released    lease live"), "{report}");
            assert_eq!(std::fs::read(&target).unwrap(), before);

            let (result, report) = import(&source, &target, false, force);
            result.unwrap();
            assert!(
                report.contains("1 released, 2 unchanged, 0 conflict(s)"),
                "{report}"
            );
            assert_eq!(
                LeaseLedger::load(&target).unwrap().get("live"),
                Some(&expected)
            );
            assert!(!expected.is_live(Utc::now()));
            let after = std::fs::read(&target).unwrap();
            let (result, report) = import(&source, &target, false, force);
            result.unwrap();
            assert!(
                report.contains("0 released, 3 unchanged, 0 conflict(s)"),
                "{report}"
            );
            assert_eq!(std::fs::read(&target).unwrap(), after);
            assert_eq!(std::fs::read(&source).unwrap(), source_before);
        }
    }

    #[test]
    fn a_source_release_for_a_different_file_remains_a_conflict() {
        for already_released in [false, true] {
            let (_dir, source, target) = setup();
            import(&source, &target, false, false).0.unwrap();
            if already_released {
                release_in_source(&target, "live", None);
            }
            let mut ledger = LeaseLedger::load(&source).unwrap();
            let mut row = ledger.get("live").unwrap().clone();
            row.file_id = "different-file".into();
            ledger.insert(row);
            ledger.save(&source).unwrap();
            release_in_source(&source, "live", None);
            let before = std::fs::read(&target).unwrap();

            let (result, report) = import(&source, &target, false, false);
            assert!(result.is_err());
            assert!(report.contains("conflict    lease live"), "{report}");
            assert_eq!(std::fs::read(&target).unwrap(), before);
        }
    }

    #[test]
    fn an_expired_but_unreleased_row_takes_the_release() {
        let (_dir, source, target) = setup();
        import(&source, &target, false, false).0.unwrap();
        release_in_source(&source, "expired", None);

        let (result, report) = import(&source, &target, false, false);

        result.unwrap();
        assert!(report.contains("released    lease expired"), "{report}");
        assert!(LeaseLedger::load(&target)
            .unwrap()
            .get("expired")
            .unwrap()
            .released_at
            .is_some());
    }

    #[test]
    fn a_diverged_lease_released_in_both_tools_is_unchanged() {
        let (_dir, source, target) = setup();
        import(&source, &target, false, false).0.unwrap();
        let mut ledger = LeaseLedger::load(&target).unwrap();
        let mut row = ledger.get("live").unwrap().clone();
        row.released_at = Some(Utc::now() - ChronoDuration::hours(1));
        row.version = "local-version".into();
        ledger.insert(row.clone());
        ledger.save(&target).unwrap();
        release_in_source(&source, "live", None);

        let (result, report) = import(&source, &target, false, true);

        result.unwrap();
        assert!(report.contains("already released in gwi"), "{report}");
        assert_eq!(LeaseLedger::load(&target).unwrap().get("live"), Some(&row));
    }

    #[test]
    fn a_release_changes_only_the_release_fields_of_gwis_row() {
        let (_dir, source, target) = setup();
        import(&source, &target, false, false).0.unwrap();
        release_in_source(&source, "live", Some("replacement"));
        let before = LeaseLedger::load(&target)
            .unwrap()
            .get("live")
            .unwrap()
            .clone();

        import(&source, &target, false, false).0.unwrap();

        let after = LeaseLedger::load(&target)
            .unwrap()
            .get("live")
            .unwrap()
            .clone();
        let mut expected = before;
        expected.released_at = after.released_at;
        expected.superseded_by = Some("replacement".to_string());
        assert!(after.released_at.is_some());
        assert_eq!(after, expected);
    }

    #[test]
    fn a_dry_run_reports_a_release_but_writes_nothing() {
        let (_dir, source, target) = setup();
        import(&source, &target, false, false).0.unwrap();
        release_in_source(&source, "live", None);
        let before = std::fs::read(&target).unwrap();

        let (result, report) = import(&source, &target, true, false);

        result.unwrap();
        assert!(report.contains("released    lease live"), "{report}");
        assert!(
            report.contains("Dry run, nothing written: 0 added, 0 overwritten, 1 released"),
            "{report}"
        );
        assert_eq!(std::fs::read(&target).unwrap(), before);
    }

    #[test]
    fn propagating_a_release_leaves_the_source_alone() {
        let (dir, source, target) = setup();
        import(&source, &target, false, false).0.unwrap();
        release_in_source(&source, "live", None);
        let before = std::fs::read(&source).unwrap();

        import(&source, &target, false, false).0.unwrap();

        assert_eq!(std::fs::read(&source).unwrap(), before);
        let entries: Vec<_> = std::fs::read_dir(dir.path().join("omni-dev"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, ["lease-ledger.jsonl"]);
    }

    #[test]
    fn a_release_is_never_undone_by_a_live_source_row() {
        let (_dir, source, target) = setup();
        import(&source, &target, false, false).0.unwrap();
        let mut ledger = LeaseLedger::load(&target).unwrap();
        let mut row = ledger.get("live").unwrap().clone();
        row.released_at = Some(Utc::now());
        ledger.insert(row.clone());
        ledger.save(&target).unwrap();

        for force in [false, true] {
            import(&source, &target, false, force).0.unwrap();
            assert_eq!(
                LeaseLedger::load(&target).unwrap().get("live"),
                Some(&row),
                "force={force}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_unopenable_source_lock_falls_back_to_reading_without_it() {
        use std::os::unix::fs::PermissionsExt;

        // Root can still open the 0o400 lock for writing, so the permission cannot bite.
        crate::test_support::skip_as_root!();

        let (_dir, source, target) = setup();
        let lock = lock_path_for(&source);
        std::fs::write(&lock, "").unwrap();
        std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o400)).unwrap();

        let (result, report) = import(&source, &target, false, false);

        result.unwrap();
        assert!(
            report.contains("warning: could not lock omni-dev's ledger"),
            "{report}"
        );
        assert_eq!(tokens(&target).len(), 3);
    }

    #[test]
    fn a_source_that_is_not_a_file_is_named_not_reported_missing() {
        let (dir, _source, target) = setup();

        let (result, _) = import(&dir.path().join("omni-dev"), &target, false, false);

        let err = result.unwrap_err().to_string();
        assert!(err.contains("not a lease ledger file"), "{err}");
    }

    #[test]
    fn a_missing_source_is_a_clean_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("gwi").join("lease-ledger.jsonl");

        let (result, report) = import(&dir.path().join("none.jsonl"), &target, false, false);

        result.unwrap();
        assert!(report.contains("no leases to import"), "{report}");
        assert!(!dir.path().join("gwi").exists());
    }

    #[test]
    fn an_empty_source_imports_nothing_and_creates_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("lease-ledger.jsonl");
        std::fs::write(&source, "\n").unwrap();
        let target = dir.path().join("gwi").join("lease-ledger.jsonl");

        let (result, report) = import(&source, &target, false, false);

        result.unwrap();
        assert!(report.contains("holds no leases"), "{report}");
        assert!(!dir.path().join("gwi").exists());
    }

    #[test]
    fn a_corrupt_source_is_an_error_that_leaves_the_target_alone() {
        let (_dir, source, target) = setup();
        std::fs::write(&source, "{\"token\": \"half a row\n").unwrap();
        ledger_of(vec![record("gwi-own", 10)])
            .save(&target)
            .unwrap();
        let before = std::fs::read(&target).unwrap();

        let (result, _) = import(&source, &target, false, false);

        let err = format!("{:#}", result.unwrap_err());
        assert!(
            err.contains("line 1") && err.contains("lease ledger"),
            "{err}"
        );
        assert_eq!(std::fs::read(&target).unwrap(), before);
    }

    #[test]
    fn an_unknown_backup_kind_refuses_import_without_touching_the_destination() {
        let fixture =
            include_str!("../../../tests/fixtures/lease-ledger-unknown-backup-kind.jsonl");
        for existing_target in [false, true] {
            for (dry_run, force) in [(false, false), (false, true), (true, false), (true, true)] {
                let dir = tempfile::tempdir().unwrap();
                let source = dir.path().join("source.jsonl");
                let target = dir.path().join("gwi").join("lease-ledger.jsonl");
                std::fs::write(&source, fixture).unwrap();
                let before = if existing_target {
                    ledger_of(vec![record("gwi-own", 10)])
                        .save(&target)
                        .unwrap();
                    Some(std::fs::read(&target).unwrap())
                } else {
                    None
                };

                let (result, _) = import(&source, &target, dry_run, force);

                let err = format!("{:#}", result.unwrap_err());
                assert!(err.contains("line 2"), "{err}");
                assert!(err.contains("unknown variant `future_archive`"), "{err}");
                if let Some(before) = before {
                    assert_eq!(std::fs::read(&target).unwrap(), before);
                    assert_eq!(tokens(&target), vec!["gwi-own"]);
                } else {
                    assert!(!target.parent().unwrap().exists());
                }
                assert_eq!(std::fs::read(&source).unwrap(), fixture.as_bytes());
            }
        }
    }

    #[test]
    fn the_report_never_names_a_backup_location() {
        let (_dir, source, target) = setup();

        let (_, report) = import(&source, &target, false, false);
        let (_, again) = import(&source, &target, true, false);

        for text in [&report, &again] {
            assert!(
                !text.contains(BACKUP_PATH) && !text.contains("file-id-1"),
                "{text}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_written_ledger_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, source, target) = setup();

        import(&source, &target, false, false).0.unwrap();

        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&target), 0o600);
        assert_eq!(mode(target.parent().unwrap()), 0o700);
    }

    #[test]
    fn the_default_source_sits_beside_gwis_state_directory() {
        let source = default_source_ledger().unwrap();
        let target = crate::drive::lease::ledger::ledger_path().unwrap();

        assert_eq!(source.file_name(), target.file_name());
        assert_eq!(source.parent().unwrap().file_name().unwrap(), "omni-dev");
        assert_eq!(
            source.parent().unwrap().parent(),
            target.parent().unwrap().parent()
        );
    }
}
