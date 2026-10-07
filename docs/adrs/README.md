# Architecture Decision Records

This directory contains the Architecture Decision Records (ADRs) for the gwi project. An ADR
captures a single significant architectural or design decision along with its context and
consequences. See [ADR-0000](adr-0000.md) for the format.

The Gmail ADRs (0063-0068 and 0079) are carried over from
[omni-dev](https://github.com/rust-works/omni-dev/tree/main/docs/adrs) with their numbers, so
existing references keep resolving. They are historical records: the commands, paths and
environment variables they quote are omni-dev's at the time (`omni-dev gmail`, `~/.omni-dev`,
`OMNI_DEV_*`), which gwi spells `gwi gmail`, `~/.gwi` and `GWI_*`. Links to ADRs that stay in
omni-dev point there. The Drive ADRs are relocated here with the Drive code.

## Status Legend

| Emoji | Status     | Meaning                               |
|-------|------------|---------------------------------------|
| 🟡    | Proposed   | Under discussion, not yet agreed upon |
| ✅    | Accepted   | Agreed and in effect                  |
| ❌    | Deprecated | No longer applies                     |
| 🔄    | Superseded | Replaced by a newer ADR               |

## Inventory

| ADR                     | Status     | Date       | Title                                                                                     |
|-------------------------|------------|------------|-------------------------------------------------------------------------------------------|
| [ADR-0000](adr-0000.md) | ✅ Accepted | 2026-10-07 | Use Architecture Decision Records                                                         |
| [ADR-0001](adr-0001.md) | 🟡 Proposed | 2026-10-07 | gwi Is Standalone: Forked Infrastructure, Own Config, Log and MCP Server                  |
| [ADR-0063](adr-0063.md) | ✅ Accepted | 2026-08-02 | OAuth2 Authorization-Code + PKCE for Gmail, with Bring-Your-Own Google Cloud Project      |
| [ADR-0064](adr-0064.md) | ✅ Accepted | 2026-08-03 | Presence-on-Disk Idempotence and Immutable-`.eml`/Mutable-Manifest Split for `gmail sync` |
| [ADR-0065](adr-0065.md) | ✅ Accepted | 2026-08-04 | `gmail sync --extract-attachments`                                                        |
| [ADR-0066](adr-0066.md) | ✅ Accepted | 2026-08-05 | A Gmail-Specific Named-Account Store, Orthogonal to `--profile`                           |
| [ADR-0067](adr-0067.md) | ✅ Accepted | 2026-08-06 | Automatic Chrome-Profile Resolution for Gmail Login, Opt-In Per Account                   |
| [ADR-0068](adr-0068.md) | ✅ Accepted | 2026-08-06 | Concurrent Multi-Account Gmail Sync via `gmail-sync.yaml` and a Shared Fetch Semaphore    |
| [ADR-0079](adr-0079.md) | ✅ Accepted | 2026-09-08 | Restoring an Archive into a Mailbox via `messages.insert`                                 |
