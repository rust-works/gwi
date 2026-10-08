# Architecture Decision Records

This directory contains the Architecture Decision Records (ADRs) for the gwi project. An ADR
captures a single significant architectural or design decision along with its context and
consequences. See [ADR-0000](adr-0000.md) for the format.

The Gmail ADRs (0063-0068 and 0079) and the Drive ADRs (0069-0071, 0073-0078, 0080-0086 and
0091-0094) are carried over from
[omni-dev](https://github.com/rust-works/omni-dev/tree/main/docs/adrs) with their numbers, so
existing references keep resolving; four Drive ADRs (0077, 0082, 0084, 0086) keep the titled
filenames they have there. They are historical records: the commands, paths and
environment variables they quote are omni-dev's at the time (`omni-dev gmail`, `omni-dev drive`,
`~/.omni-dev`, `OMNI_DEV_*`), which gwi spells `gwi gmail`, `gwi drive`, `~/.gwi` and `GWI_*`.
Links to ADRs and docs that stay in omni-dev point there, among them the secret-handling ADRs
0089 and 0090.

## Status Legend

| Emoji | Status     | Meaning                               |
|-------|------------|---------------------------------------|
| 🟡    | Proposed   | Under discussion, not yet agreed upon |
| ✅    | Accepted   | Agreed and in effect                  |
| ❌    | Deprecated | No longer applies                     |
| 🔄    | Superseded | Replaced by a newer ADR               |

## Inventory

| ADR                                                     | Status     | Date       | Title                                                                                      |
|---------------------------------------------------------|------------|------------|--------------------------------------------------------------------------------------------|
| [ADR-0000](adr-0000.md)                                 | ✅ Accepted | 2026-10-07 | Use Architecture Decision Records                                                          |
| [ADR-0001](adr-0001.md)                                 | 🟡 Proposed | 2026-10-07 | gwi Is Standalone: Forked Infrastructure, Own Config, Log and MCP Server                   |
| [ADR-0063](adr-0063.md)                                 | ✅ Accepted | 2026-08-02 | OAuth2 Authorization-Code + PKCE for Gmail, with Bring-Your-Own Google Cloud Project       |
| [ADR-0064](adr-0064.md)                                 | ✅ Accepted | 2026-08-03 | Presence-on-Disk Idempotence and Immutable-`.eml`/Mutable-Manifest Split for `gmail sync`  |
| [ADR-0065](adr-0065.md)                                 | ✅ Accepted | 2026-08-04 | `gmail sync --extract-attachments`                                                         |
| [ADR-0066](adr-0066.md)                                 | ✅ Accepted | 2026-08-05 | A Gmail-Specific Named-Account Store, Orthogonal to `--profile`                            |
| [ADR-0067](adr-0067.md)                                 | ✅ Accepted | 2026-08-06 | Automatic Chrome-Profile Resolution for Gmail Login, Opt-In Per Account                    |
| [ADR-0068](adr-0068.md)                                 | ✅ Accepted | 2026-08-06 | Concurrent Multi-Account Gmail Sync via `gmail-sync.yaml` and a Shared Fetch Semaphore     |
| [ADR-0069](adr-0069.md)                                 | ✅ Accepted | 2026-08-15 | A Drive-Specific Named-Account Store and Read-Only OAuth2 Client, Mirroring Gmail's Design |
| [ADR-0070](adr-0070.md)                                 | ✅ Accepted | 2026-08-18 | Security-Gated Rename/Move for the Drive Integration                                       |
| [ADR-0071](adr-0071.md)                                 | ✅ Accepted | 2026-08-25 | Folder-Scoped Write Permissions for the Drive Integration                                  |
| [ADR-0073](adr-0073.md)                                 | ✅ Accepted | 2026-09-06 | Google Sheets API Support for the Drive Integration                                        |
| [ADR-0074](adr-0074.md)                                 | ✅ Accepted | 2026-09-06 | File-Id-Keyed Write-Permission Rules                                                       |
| [ADR-0075](adr-0075.md)                                 | ✅ Accepted | 2026-09-07 | Structural Sheet Edits via `spreadsheets.batchUpdate`                                      |
| [ADR-0076](adr-0076.md)                                 | ✅ Accepted | 2026-09-07 | Google Docs Text Mutation Behind a Revision Lease                                          |
| [ADR-0077](adr-0077-sheets-deletion-via-batchupdate.md) | ✅ Accepted | 2026-09-08 | Sheet, Row, Column and Range Deletion via `spreadsheets.batchUpdate`                       |
| [ADR-0078](adr-0078.md)                                 | ✅ Accepted | 2026-09-08 | Sheet Formatting, Data Validation and Protected Ranges via `spreadsheets.batchUpdate`      |
| [ADR-0079](adr-0079.md)                                 | ✅ Accepted | 2026-09-08 | Restoring an Archive into a Mailbox via `messages.insert`                                  |
| [ADR-0080](adr-0080.md)                                 | ✅ Accepted | 2026-09-10 | Touch ID-Authorised Backup Leases and a Fail-Closed Audit Log for Drive Writes             |
| [ADR-0081](adr-0081.md)                                 | ✅ Accepted | 2026-09-20 | Permission-Gate Mapping for the Second Sheets Capability Tranche                           |
| [ADR-0082](adr-0082-banded-ranges.md)                   | ✅ Accepted | 2026-09-21 | Banded Ranges via `spreadsheets.batchUpdate`                                               |
| [ADR-0083](adr-0083.md)                                 | ✅ Accepted | 2026-09-21 | Permission-Gate Mapping for the Grid-Mutation `batchUpdate` Verbs                          |
| [ADR-0084](adr-0084-dimension-groups.md)                | ✅ Accepted | 2026-09-21 | Dimension Groups (Row/Column Outlining) via `spreadsheets.batchUpdate`                     |
| [ADR-0085](adr-0085.md)                                 | ✅ Accepted | 2026-09-21 | Sheet View Properties via `updateSheetProperties`                                          |
| [ADR-0086](adr-0086-workbook-properties.md)             | ✅ Accepted | 2026-09-22 | Workbook Properties via `updateSpreadsheetProperties`                                      |
| [ADR-0091](adr-0091.md)                                 | ✅ Accepted | 2026-10-03 | Gated Sheets and Docs Content Writes over MCP                                              |
| [ADR-0092](adr-0092.md)                                 | ✅ Accepted | 2026-10-02 | Gated Trash and Restore for Individual Drive Files                                         |
| [ADR-0093](adr-0093.md)                                 | ✅ Accepted | 2026-10-02 | Google Slides object reads and guarded text replacement                                    |
| [ADR-0094](adr-0094.md)                                 | ✅ Accepted | 2026-10-02 | Anchor-Addressed Docs Insertion and Deletion                                               |
