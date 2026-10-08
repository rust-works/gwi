# Request and audit logs

> Issue numbers such as `#1664` in this document are
> [omni-dev](https://github.com/rust-works/omni-dev/issues) issues, where the logging and
> Drive code was written; gwi's own issues are linked with `rust-works/gwi`.

gwi keeps two local, append-only JSON Lines files and ships a `gwi log` command to search and
pretty-print them:

| | Request log | Audit log |
|---|---|---|
| File | `log.jsonl` | `audit.jsonl` |
| Records | One `invocation` record per run, one `http` record per outbound request, and the Drive mutation attempts | The leased-write lifecycle and refusal trail |
| On a write failure | Swallowed; the command's exit code is unaffected | Propagated: an operation whose audit record cannot be written does not happen |
| `GWI_LOG_DISABLE=1` | Suppresses all writes | No effect |
| Rotation (`GWI_LOG_MAX_SIZE`) and `gwi log prune` | Applies | Never applies; `gwi log prune --audit` is refused |
| Path override | `GWI_LOG_FILE` | `GWI_AUDIT_LOG_FILE` |
| Read with | `gwi log` | `gwi log --audit` |

The request log is the durable, queryable record of *what was run* and *what it talked to over
the network*, which `RUST_LOG` tracing (ephemeral, stderr only) is not. The audit log exists
for the opposite guarantee: it is the forensic trail for Drive writes made under a lease, so
it is fail-closed and exempt from every growth bound.

**Contents**

1. [Location](#location)
2. [Environment variables](#environment-variables)
3. [What gets recorded](#what-gets-recorded)
4. [The `drivemutation` record](#the-drivemutation-record)
5. [Audit log](#audit-log)
6. [`gwi log`](#gwi-log)
7. [Bounding growth](#bounding-growth)
8. [Redaction posture](#redaction-posture)
9. [Schema and compatibility](#schema-and-compatibility)

## Location

Both files sit side by side under the platform state directory, `<state dir>/gwi/`, resolved
as:

1. `GWI_LOG_FILE` (request log) or `GWI_AUDIT_LOG_FILE` (audit log), if set and non-empty.
2. Otherwise the platform state directory joined with `gwi/log.jsonl` or `gwi/audit.jsonl`:
   `~/.local/state/gwi/` on Linux.
3. On a platform with no state directory (macOS), the data directory instead:
   `~/Library/Application Support/gwi/`.

The directory is created `0700` and the files `0600` (a file that already exists is tightened
to `0600` on every open). The logs never leave your machine. They are separate from
omni-dev's logs, which `gwi import` does not copy; that history stays where it was
([ADR-0001](adrs/adr-0001.md)).

Pointing both overrides at one file is refused when the path is resolved, however the two
are spelled (relative, `..`, symlink), so neither a best-effort append, rotation nor
`gwi log prune` can reach `audit.jsonl` through an aliasing `GWI_LOG_FILE`. The request log
then has no path at all: nothing is written to it, a warning is logged through `tracing`, and
`gwi log` fails with "could not resolve the log file path".

## Environment variables

| Variable | Effect |
|---|---|
| `GWI_LOG_FILE` | Override the request log path. |
| `GWI_LOG_DISABLE=1` | Disable the request log entirely. The audit log is unaffected. |
| `GWI_LOG_BODIES=1` | Opt in to recording request and response bodies. Off by default: payloads are large and a Drive body can hold the content of your files. |
| `GWI_LOG_HEADERS=1` | Opt in to recording request and response headers (redacted; see [Redaction posture](#redaction-posture)). |
| `GWI_LOG_MAX_SIZE` | Enable size-capped rotation on write, for example `10mb` (unix only; see [Automatic size-capped rotation](#automatic-size-capped-rotation)). |
| `GWI_LOG_KEEP_FILES` | Number of rotated files to keep when rotation is enabled (default `3`). |
| `GWI_AUDIT_LOG_FILE` | Override the **audit** log path. None of the other variables above affect the audit log. |

The truthy variables (`GWI_LOG_DISABLE`, `GWI_LOG_BODIES`, `GWI_LOG_HEADERS`) are read on
every write, so they must be present in the environment of whatever writes the log: your
shell for a CLI run, or the environment `gwi-mcp` was started with for an MCP tool call.

Request logging is **best effort**: a write failure is swallowed (logged only at
`tracing::debug`) and can never change a command's exit code. The audit log is the deliberate
exception; see [Audit log](#audit-log).

## What gets recorded

One JSON object per line. A `kind` field says which of four record types it is, so the
request log is a complete invocation history, not only an HTTP history:

- **`kind: "invocation"`**: one per process run, and one per MCP tool call. It holds the
  resolved subcommand path (`command`), the full argv (`command_line`, with secret-bearing
  values redacted), `exit_code`, `duration_ms`, any top-level `error`, and a snapshot of the
  `GWI_*` environment variables (`env`, with secret-looking values redacted).
- **`kind: "http"`**: one per outbound request, recorded *inside* each client's retry loop, so
  retries and transport failures are captured too. It holds the `service`, `method`, `url`
  (secret-bearing query and fragment values redacted), `status_code` (absent on a transport
  error), `elapsed_ms` and any `error`. The `service` tag is `gmail` for Gmail and `drive`
  for Drive, Docs, Sheets and Slides alike; the Google host stays visible in `url`. The OAuth
  token-endpoint calls of both `auth login` flows are recorded too.
- **`kind: "drivemutation"`**: one per Drive, Docs, Sheets or Slides mutation attempt, whether
  it succeeded, was refused by the write gate before any API call, or failed. See
  [The `drivemutation` record](#the-drivemutation-record).
- **`kind: "audit"`**: written to the separate `audit.jsonl`, never to this file. See
  [Audit log](#audit-log).

Every `http` and `drivemutation` record shares an `invocation_id` with the invocation that
issued it, so a single `--id` pulls a run and everything it did. The `source` field says
what drove the run: `cli` for a `gwi` command, `mcp` for a `gwi-mcp` tool call (with the tool
name in `mcp_tool`). Each MCP tool call is its own invocation, with its own `invocation_id`.

`--dry-run` previews are never logged as mutations: a mutation record is written only for a
real attempt.

## The `drivemutation` record

A `drivemutation` record is written from *inside* the mutation itself, so it covers every
caller (the CLI and the MCP tools) and not just one front end. It is tagged `service:
"drive"`, and `command` is `["drive", "<operation>"]`, where `<operation>` is the verb's name,
which is not always the CLI spelling: `gwi drive sheets write` is operation `sheets-write`.
The operations are:

| Family | Operations |
|---|---|
| Files | `create`, `upload`, `edit`, `rename`, `move`, `trash`, `untrash` |
| Lease restore | `lease-restore` (the restore write; the lease's own events are in the [audit log](#audit-log)) |
| Docs | `docs-create`, and the `docs-*` text, table, list, style and named-range verbs, for example `docs-replace`, `docs-append`, `docs-insert`, `docs-delete` |
| Sheets | `sheets-create`, and the `sheets-*` cell, structure, formatting, protection, filter, chart, banding, named-range, pivot and cleanup verbs, for example `sheets-write`, `sheets-insert-rows`, `sheets-format-cells`, `sheets-delete-sheet` |
| Slides | `slides-replace` |

The outcome is in the free-form `context` map. `file_id`, `file_name` and `status` are always
present; every other key is **omitted when it does not apply**.

`status` is the domain outcome, kebab-case, and varies by verb. Examples are `blocked` (the
write gate refused it, so no mutating API call happened, which is itself the
security-relevant event), `failed`, `stale-revision` (a Docs `412` on the revision lease),
the lease refusals `refused-no-lease`, `refused-lease-expired`, `refused-lease-wrong-file`
and `refused-lease-stale`, and `applied-reply-unreadable` (the API answered 2xx but the reply
could not be parsed; the mutation happened, so inspect before retrying).

| Key | Set by | Meaning |
|---|---|---|
| `file_id`, `file_name`, `status` | every verb | The file acted on (its name at the time) and the outcome. |
| `added_principals`, `removed_principals` | `move` | Comma-separated principals gaining or losing access. |
| `crosses_drive_boundary` | `move` | `true` when the file crossed a My Drive / Shared Drive boundary. |
| `resolved_folder_id` | gated verbs | The folder the write-permission gate evaluated against (`--parent` for `create` and `upload`, the target's parent for `edit`). Absent for the ungated `rename` and `move`. |
| `decided_by_folder_id`, `decided_by_depth` | gated verbs | The configured folder rule that decided the verdict and how many levels above `resolved_folder_id` it sits. |
| `decided_by_file_id` | gated verbs | The configured **file** rule that decided the verdict. Mutually exclusive with `decided_by_folder_id` on purpose, so `--query decided_by_folder_id:<id>` can never start matching a file id. A file rule matches the target itself and walks no chain, so `decided_by_depth` and `resolved_folder_id` are absent alongside it. |
| `range` | Sheets writes | The A1 range as composed and sent. For `text-to-columns` it is the source column, not the spill span. |
| `updated_range`, `updated_rows`, `updated_columns`, `updated_cells` | Sheets writes | What the API reported changing. `updated_range` can differ from `range`, since the server resolves an open-ended range against the sheet's real extent. |
| `sheet_id`, `sheet_title` | Sheets structural verbs | The stable numeric id the API addresses (it survives a later rename, so it is the only durable answer to "which tab") and the title at the time. Absent for `update-workbook-properties`, which has no sheet target. |
| `sheet_new_title` | `rename-sheet` | The title the sheet was renamed *to*; `sheet_title` holds the old one. |
| `dimension_range` | row and column verbs | For example `ROWS 5:7`, 1-based inclusive like the CLI's `--at`. For `move-rows` and `move-columns` it is the *source* span. |
| `move_to` | `move-rows`, `move-columns` | The raw 1-based `--before` value (the destination). |
| `grid_range` | `delete-range`, `insert-range` | A rectangle such as `rows 2-4, columns 2-3`, 1-based inclusive. Only `operation` tells a destructive `delete-range` from an `insert-range`. |
| `fields_changed` | formatting, properties, chart, filter and fill verbs | A human-readable summary of what the verb set or did. Where a dry run prints the same summary it is tense-neutral. For chart and slicer verbs that resolve an existing object it has the object's type, title and anchor appended. |
| `discarded_cells` | `merge-cells` | The non-top-left, non-blank cells a merge discarded, as `A1: value` strings joined with `; `. |
| `overwritten_cells`, `overwritten_cells_are_upper_bound` | `auto-fill`, `text-to-columns` | The non-blank destination cells that may have been overwritten, as bare A1 addresses (never their values). The second key is `true` only when the list is an upper bound; absent means it is exact. It is always `true` for `text-to-columns`. |
| `occurrences_changed` | find-and-replace verbs | What the *server* reported changing, which can differ from the client-side dry-run estimate. |
| `inserted_chars` | Docs text verbs | The number of characters inserted. |
| `required_revision_id` | Docs text verbs | The revision lease presented, so a `stale-revision` refusal is as auditable as a success. An opaque, short-lived id, not a secret. |
| `validation_type` | `set-data-validation`, `clear-data-validation` | The condition type applied, or `cleared`. |
| `protected_range_id` | protection verbs | The stable id of the protected range (server-assigned for `protect-range`). |
| `protection_editors_added`, `protection_editors_removed` | protection verbs | Comma-separated editors granted or removed an exemption. |
| `filter_view_id` | filter-view verbs | The stable id of the filter view. Absent for `set-basic-filter` and `clear-basic-filter`, which use `sheet_id`. |
| `embedded_object_id` | chart and slicer verbs | The stable id of the chart or slicer acted on (one key for both, since the API addresses both by `objectId`). |
| `banded_range_id` | banding verbs | The stable id of the banded range. |
| `named_range_id` | named-range verbs | The stable id of the named range. |
| `referencing_formula_count`, `referencing_formula_locations` | `delete-named-range` | How many formulas reference the name being removed and their A1 locations as `Sheet!A1` strings, `; `-joined. Never the formula text or a cell value. |

**Text is never recorded in these fields.** The searched, replacement, appended, inserted and
deleted text (and anchor text) is user prose, often the most sensitive thing in the
invocation, and the Sheets cell values written, discarded or overwritten are likewise not
logged, only their addresses and counts. The `--find` and `--replacement` argv values are
redacted in the invocation record as well. A structural Sheets verb reads and logs no cell
content at all.

The operations are specified in the ADRs: rename and move in [ADR-0070](adrs/adr-0070.md),
the folder write gate in [ADR-0071](adrs/adr-0071.md) and its file-id rules in
[ADR-0074](adrs/adr-0074.md), Sheets cell writes in [ADR-0073](adrs/adr-0073.md), structural
edits in [ADR-0075](adrs/adr-0075.md), Docs text in [ADR-0076](adrs/adr-0076.md), deletion in
[ADR-0077](adrs/adr-0077-sheets-deletion-via-batchupdate.md), formatting, validation and
protection in [ADR-0078](adrs/adr-0078.md), the gate mapping for the second Sheets tranche
(filters, charts, slicers, named ranges) in [ADR-0081](adrs/adr-0081.md), banding in
[ADR-0082](adrs/adr-0082-banded-ranges.md), the grid-mutation verbs in
[ADR-0083](adrs/adr-0083.md), dimension groups in
[ADR-0084](adrs/adr-0084-dimension-groups.md), sheet properties in
[ADR-0085](adrs/adr-0085.md) and workbook properties in
[ADR-0086](adrs/adr-0086-workbook-properties.md). The write gate itself is in
[drive.md](drive.md#write-permissions).

## Audit log

`audit.jsonl` is a **separate** file, a sibling of `log.jsonl`, using the same record schema
and the same `gwi log` reader (pass `--audit`). Everything in the request-log sections above
stays as described regardless of its existence: a leased Drive write still produces its
ordinary `drivemutation` record in `log.jsonl`, correlated to its `audit` record by the
shared `invocation_id`.

Where `log.jsonl` is best effort, prunable and can be disabled outright, `audit.jsonl` is
**fail-closed** ([ADR-0080](adrs/adr-0080.md) §11): an operation whose write-ahead audit
record cannot be written does not happen. Each line is `fsync`ed (and the directory too, when
the file is new) before the mutating call it precedes, which `GWI_LOG_DISABLE` cannot skip.
The file is exempt from `GWI_LOG_MAX_SIZE` rotation however `GWI_LOG_FILE` is spelled, and
`gwi log prune` refuses it, whether through `--audit` or through a `GWI_LOG_FILE` that
resolves to it.

Every record carries `kind: "audit"`, `service: "drive"` and, in `context`, an `integration`
key (`drive` today, so a later integration's trail is additive rather than a rename), a
`file_id` and a `verdict`. The other `context` keys appear when they apply: `lease_id` (the
token), `version_before`/`version_after`, `modified_time_before`/`modified_time_after`,
`backup_location` (a local path for a byte backup, or the backup copy's own file id for a
native-document backup), `backup_sha256`, `backup_size`, `auth_policy` (`device-owner` or
`biometrics-only`), `restored_from_lease_id` and `superseded_lease_id`. A failure is in the
record's `error`.

`command` is `["drive", "<verb>"]` with the same operation name as the matching
`drivemutation` record, so `--query 'command:sheets-delete-sheet'` matches both files.

**`drive lease acquire`** writes one record per attempt, `command: ["drive",
"lease-acquire"]`, whatever the outcome. `verdict` is `acquired`, `acquired-headless-waiver`
(it proceeded under the headless opt-out, no human prompted), `already-leased`,
`refused-native-document`, `refused-concurrent-change` (the file changed while its backup was
being taken, so no lease can vouch for that backup), `denied`, `unavailable` or `failed`. The
`already-leased`, `refused-concurrent-change` and `failed` verdicts take a `-backup-orphaned`
suffix when the attempt took a backup and then failed to reclaim it. This record is
best effort: acquiring mutates no Drive content, so a logging failure is warned and does not
turn a successful acquisition into a reported failure.

**A leased write** produces, in `audit.jsonl`:

- For a refusal that never reaches the mutating call, one best-effort record whose `verdict`
  is `refused-no-lease`, `refused-lease-expired`, `refused-lease-wrong-file` or
  `refused-lease-stale`, or `failed` when the ledger lock could not be taken.
- For a lease that checks out, a **write-ahead** `pending` record carrying `lease_id`,
  `version_before` and `modified_time_before`, then, after the call, a best-effort outcome
  record with the same `lease_id`: `allowed` (with `version_after` and
  `modified_time_after`) or `failed` (with the API `error`, including a Docs `412` on the
  revision lease, which the CLI reports as `stale-revision`).

A `pending` record with no matching outcome means the write was interrupted midway, which is
the point of writing the former durably first.

**`drive lease restore <TOKEN>`** writes its own record, `command: ["drive", "lease-restore"]`,
in addition to the `pending`/outcome pair its restore write produces under the fresh lease.
`lease_id` is the fresh token and `restored_from_lease_id` is always the backup lease's token,
so `--query 'restored_from_lease_id:<token>'` finds every restore attempt made from one
backup. `verdict` is one of `restored`, `restored-sheet`, `restored-headless-waiver`,
`restored-sheet-headless-waiver`, `sheet-already-restored`, `no-such-backup-token`,
`no-typed-restore-path`, `backup-too-large-for-simple-upload`, `refused-no-visible-parents`,
`blocked`, `already-leased`, `refused-native-document`, `refused-concurrent-change`, `denied`,
`unavailable`, `failed` or `fresh-lease-but-write-failed`. A restore supersedes the lease it
restores from: the `acquired` record of its fresh lease carries `superseded_lease_id`.

**`drive lease release <TOKEN>`** writes one record, `command: ["drive", "lease-release"]`,
with `lease_id` the token presented (even on a refusal) and `verdict` `released`,
`release-not-live`, `release-no-such-token` or `failed`. A release makes no Drive call, so
there is no `drivemutation` record to join against.

**`drive lease prune`** writes one best-effort record per row, `command: ["drive",
"lease-prune"]`, with `verdict` `pruned` or `prune-failed` (with the `error`), so a removed
backup is discoverable from the log and not only from the command's summary.

## `gwi log`

```
gwi log [OPTIONS]          # search (default)
gwi log prune [OPTIONS]    # trim the request log; see Bounding growth
```

With no subcommand, `gwi log` searches the log and prints the records that match every
filter given. A search flag placed before `prune` is refused rather than silently ignored.
`gwi log --help` lists every option.

### Filters

| Flag | Matches |
|---|---|
| `--since <DUR_OR_TS>` | Lower time bound: a relative window (`45s`, `30m`, `2h`, `1d`, `1w`), a date (`2026-07-01`, midnight UTC) or an RFC3339 timestamp. |
| `--until <DUR_OR_TS>` | Upper time bound, in the same forms (a relative value means that long ago). Pair with `--since` for a bounded window. |
| `--method <METHOD>` | HTTP method, case-insensitive. |
| `--status <STATUS>` | An exact code (`200`), a class (`5xx`), a comma list (`4xx,5xx`), a comparison (`>=400`), or a `drivemutation` status such as `blocked` (exact, case-insensitive; a comma list is accepted). The same values as `status:` in `--query`. |
| `--service <NAME>` | The service tag: `gmail` or `drive`. |
| `--command <PATH>` | Resolved command-path prefix on whole segments, for example `"gmail read"`. |
| `--url <SUBSTR>` | Substring of the request URL. |
| `--grep <REGEX>` | Regular expression against the raw JSON line. |
| `--fuzzy <TOKEN>` | Substring of the raw line; repeatable, AND-ed. |
| `--query <EXPR>` | A query expression (below); repeatable, AND-ed. |
| `--id <ID>` | This record's `id` **or** `invocation_id`, which pulls a run and its requests. |

### Output

| Flag | Effect |
|---|---|
| `-o, --output <oneline\|json\|full>` | `oneline` (the default), `json` (the stored line verbatim, so it composes with `jq`) or `full` (a labelled block per record). |
| `-n, --limit <N>` | Show at most the N most recent matching records. |
| `-f, --follow` | Tail the log, printing new matching records as they are appended. |
| `--audit` | Read `audit.jsonl` instead of `log.jsonl`. Every filter, the `--query` language and all three output formats apply unchanged; only the file differs. |

A missing log file is not an error: nothing is printed. A line that does not parse as a
record (including a partly written trailing line) is skipped. Piping into something that
closes early, such as `| head`, ends the scan cleanly.

`--follow` keeps working across a `gwi log prune` or a rotation: it notices the log being
replaced by its device and inode on unix, and only by its shrinking elsewhere, and restarts
from the top of the new file.

### The `--query` language

- **Structured terms** are `field:value`. The built-in fields, with their aliases, are
  `kind`, `source`, `service`, `method`, `status`, `command` (`cmd`), `url`, `id`,
  `invocation_id` (`inv`), `mcp_tool` (`tool`), `via_daemon`, `error` (`err`), `exit_code`
  (`exit`), `duration_ms` (`duration`, `dur`), `elapsed_ms` (`elapsed`), `hostname` (`host`),
  `system_user` (`user`), `cwd` and `auth_principal` (`principal`). A flag and its field are
  the same matcher, so `--status 5xx` and `status:5xx` behave identically.
- **`status` is kind-aware.** For every kind but `drivemutation` it matches the HTTP
  `status_code` (a class or a comparison). A `drivemutation` record has no `status_code`; its
  domain status lives in `context`, so `status:` there matches it by exact, case-insensitive
  equality: `kind:drivemutation status:blocked`.
- **Numeric fields** (`exit_code`, `duration_ms`, `elapsed_ms` and `status`) take a leading
  comparator: `>`, `>=`, `<`, `<=`, or a bare `=` or number for equality. A record without
  the field never matches.
- **Text fields** (`url`, `hostname`, `system_user`, `cwd`, `auth_principal`, `error`) match a
  case-insensitive substring. `service`, `method`, `kind` and `source` match exactly,
  ignoring case.
- **Context fields.** Any other field name falls back to the record's `context` map
  (case-insensitive substring), so `file_id:<id>`, `decided_by_folder_id:<id>`,
  `verdict:acquired` and `lease_id:<token>` all work. A fallback cannot tell a typo from a
  real key, so when no scanned record has the key at all, a warning on stderr names it and
  suggests the closest built-in field or key it has seen. The exit code and stdout are
  unchanged.
- **Bare tokens** are fuzzy, case-insensitive substring matches against the raw JSON line.
- **Operators** are `AND` (also implicit between adjacent terms), `OR`, `NOT` (or a leading
  `-`) and parentheses. Quote a value that contains spaces with `"quotes"`. A word that is
  exactly one quoted string is always searched for as text, never read as an operator or a
  field, so `--query '"not"'` finds the word `not`.

```bash
gwi log --query 'kind:http AND (status:5xx OR method:POST)'
gwi log --query 'service:drive -status:2xx'              # drive requests that did not 2xx
gwi log --query 'elapsed:>1000'                          # requests slower than a second
gwi log --query 'kind:invocation exit_code:>0'           # failed runs
gwi log --query 'kind:drivemutation status:blocked'      # writes the gate refused
```

### Examples

```bash
# The last 20 things you ran.
gwi log -n 20

# Server errors in the last two hours.
gwi log --since 2h --status 5xx

# A bounded historical window.
gwi log --since 2026-07-01 --until 2026-07-02

# A run and every request and mutation it made.
gwi log --id 0001718000000-0a1b2c3d4e5f6071

# Every write that touched one file.
gwi log --query 'file_id:<FILE_ID>' -o full

# The write-lease trail for the last day.
gwi log --audit --since 1d -o full

# Compose with jq for anything not directly expressible.
gwi log -o json --service drive | jq 'select(.status_code == 429)'

# Follow live.
gwi log -f --service gmail
```

## Bounding growth

The request log is on by default for every invocation and every outbound request, so on an
active machine it grows steadily. Two opt-in bounds keep it in check; both are off by
default, so nothing changes unless you ask. Neither applies to the audit log.

### `gwi log prune`

Trims the request log in place, by age, by size or both:

```
gwi log prune [--older-than <DUR>] [--max-size <SIZE>] [--dry-run]
```

| Flag | Effect |
|---|---|
| `--older-than <DUR>` | Remove records older than a relative window (`7d`, `24h`, `2w`, `45m`). |
| `--max-size <SIZE>` | After age pruning, drop the **oldest** records until the file is at most `<SIZE>` (`10mb`, `512kb`, or a bare byte count). |
| `--dry-run` | Report what would be removed without modifying the file. |
| `--audit` | Always refused: the audit log is exempt from pruning by design. |

At least one of `--older-than` and `--max-size` is required. Sizes are binary (`kb`, `mb` and
`gb` are 1024-based). A record with a missing or unparseable timestamp, and a line that is
not a record, is kept (age pruning removes only what it can positively date as old), and
`--max-size` always keeps at least the single most recent record. The command reports the
records removed and kept and the size before and after.

```bash
# Keep the last 30 days; preview first.
gwi log prune --older-than 30d --dry-run
gwi log prune --older-than 30d

# Cap the file at 20 MB, dropping the oldest records to fit.
gwi log prune --max-size 20mb
```

Pruning rewrites the file atomically (a same-directory temporary file is renamed over the
original, keeping mode `0600`), so a concurrent reader never sees a half-written file. A
prune that removes nothing leaves the file untouched. It is not locked against concurrent
**writers**, though: a record appended during the rewrite may be lost, and, because `prune` is
itself a logged invocation, pruning the active log appends one new record of its own. For
exact accounting, prune with `GWI_LOG_DISABLE=1` or while the log is idle.

### Automatic size-capped rotation

Set `GWI_LOG_MAX_SIZE` (for example `10mb`) to rotate on write: before an append that would
push the file past the cap, `log.jsonl` is renamed to `log.jsonl.1` (shifting any existing
`log.jsonl.1` to `.2`, and so on) and a fresh `log.jsonl` is started. `GWI_LOG_KEEP_FILES`
(default `3`) bounds how many rotated files are kept; the oldest beyond that is deleted, and
`0` keeps no history. Total use is therefore roughly `(GWI_LOG_KEEP_FILES + 1) ×
GWI_LOG_MAX_SIZE`.

Rotation is **unix only** and **best effort**. When it is enabled, writers serialise on a
stable `log.jsonl.lock` file (created `0600`) for the check-rotate-append sequence, and a
rotation failure falls back to appending without rotating rather than dropping the record. A
set-but-invalid `GWI_LOG_MAX_SIZE` (or `0`) is ignored, logged at `tracing::debug`, and leaves
rotation off. `gwi log` reads only the live `log.jsonl`, so a search does not see records
that have been rotated into `log.jsonl.N`; read those files directly or with `GWI_LOG_FILE`
pointed at one.

## Redaction posture

No secret material is written, under any code path:

- **Headers** are redacted centrally before writing, and only with `GWI_LOG_HEADERS=1`. A
  header is redacted if its lowercased name is on a fixed list (`authorization`,
  `proxy-authorization`, `cookie`, `set-cookie`, `x-api-key`, and similar) or contains
  `auth`, `token`, `secret`, `key`, `cookie`, `password`, `session`, `signature` or
  `credential`. Only a non-secret `auth_principal` identity is ever kept.
- **URL query and fragment values** under a secret-looking key are replaced with `REDACTED`:
  keys ending in `token`, `secret`, `password`, `passwd`, `signature`, `apikey`, `api_key` or
  `api-key`; the exact keys `sig`, `sas`, `jwt` and `auth`; and the `X-Amz-*` and `X-Goog-*`
  signed-URL families. The host, path and parameter keys are kept, so `--url` filtering still
  works.
- **Bodies** are opt-in via `GWI_LOG_BODIES=1`.
- **The `GWI_*` environment snapshot** in an invocation record redacts any variable whose name
  contains `TOKEN`, `SECRET`, `KEY`, `PASSWORD` or `PASSWD`.
- **Argv** in the invocation record is scrubbed in both `--flag value` and `--flag=value`
  forms: a `--header` value naming a sensitive header is redacted keeping the name
  (`Authorization: REDACTED`), an inline `--body` is redacted (an `@file` reference is
  kept), `--find` and `--replacement` are redacted, and any flag whose name has a `token`,
  `secret`, `password`, `passwd` or `key` segment has its value redacted (a flag ending in
  `-file` or `-path` carries a path and is exempt). Every argv element is then run through the
  same URL redaction as above, so a secret in a URL argument is redacted too; benign argv
  passes through byte-identical.

### What redaction does not cover: file and message content

These guarantees keep *secret material* out of the log; they do not make *content* secret.
With `GWI_LOG_BODIES=1`, a Drive, Docs, Sheets, Slides or Gmail request or response body is
recorded as it is, and that is your file or message content. `RUST_LOG=debug` tracing is
ephemeral and stderr only, but if you redirect it to a file or paste it into a bug report, what
it printed goes with it. No credential appears on any of these paths: this is a
data-sensitivity note, not a credential leak.

## Schema and compatibility

Records are read and written through a single forward-compatible struct: every field is
`#[serde(default)]` and every optional field is omitted when empty. A newer reader never
chokes on an older line, and an older reader never chokes on a newer one. An unknown
`kind` or `source` reads as `unknown` rather than failing the read. The record `id` is
time-sortable (13 digits of epoch milliseconds, a dash, 16 hex digits), so sorting by `id`
is sorting by time.

| Field | On | Meaning |
|---|---|---|
| `id` | all | This record's id. |
| `invocation_id` | all | Shared by an invocation and every record it spawned. |
| `kind` | all | `invocation`, `http`, `drivemutation` or `audit`. |
| `timestamp` | all | RFC3339 with milliseconds, UTC. |
| `hostname`, `pid`, `gwi_version`, `cwd`, `system_user` | all | Where, by what and by whom it was written. |
| `command` | invocation, drivemutation, audit | The resolved subcommand path, or `["drive", "<operation>"]`. |
| `command_line` | invocation | The full argv, scrubbed. |
| `exit_code`, `duration_ms` | invocation, drivemutation (`duration_ms`) | Process exit code and wall time. |
| `env` | invocation | The redacted `GWI_*` snapshot. |
| `source` | invocation, http, drivemutation, audit | `cli` or `mcp`. |
| `mcp_tool` | when `source` is `mcp` | The tool name that drove the run. |
| `service` | http, drivemutation, audit | `gmail` or `drive`. |
| `method`, `url`, `status_code`, `elapsed_ms` | http | The request and its result. |
| `auth_principal` | http | The non-secret identity the request used. |
| `request_headers`, `response_headers` | http | Redacted, and only with `GWI_LOG_HEADERS=1`. |
| `request_body`, `response_body` | http | Only with `GWI_LOG_BODIES=1`. |
| `context` | http, drivemutation, audit | The free-form key/value map described above. |
| `error` | all | The top-level error chain, the per-request error, or the failure of a mutation or audit event. |

`via_daemon` and `daemon_session_id` are part of the schema and are readable with
`--query via_daemon:true`, but gwi has no daemon, so they are never set.
