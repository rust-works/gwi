# MCP description audit (#142)

This audit covers the 23 tools advertised by `gwi-mcp`, including every top-level
input parameter (90 parameter occurrences), against [STYLE-0029](STYLE_GUIDE.md#style-0029-mcp-tool--parameter-description-checklist).
Descriptions were compared with handlers, enum parsers, account resolution,
content-input helpers, Sheets A1 composition and write/lease engines. The generated
`tools/list` response was also inspected; source comments alone are insufficient
because explicit schema descriptions can override them.

## Findings and changes

| Tool | Top-level parameters reviewed | Result |
| --- | --- | --- |
| `gmail_account_list` | none | Added `{}` example and YAML output guidance; account discovery already clear. |
| `gmail_auth_status` | account | Added account example and YAML output guidance; credential-presence vs CLI verification distinction retained. |
| `gmail_search` | query, limit, enrich, concurrency, account | Existing query examples, numeric/boolean defaults, clamp and pagination cap satisfy the applicable items. |
| `gmail_message_read` | message_id, format, output_file, account | Added API message ID shape and search discovery field, excluded thread/draft/header IDs, added tool example and siblings, path example and inline omission behavior. |
| `gmail_thread_read` | thread_id, account | Added conversation ID example, `threadId`/enriched `thread_id` discovery sources, message/draft distinction, tool example and single-message sibling. |
| `gmail_label_list` | account | Added account example; label mutation remains CLI-only. |
| `gmail_draft_list` | query, limit, account | Existing examples, omission behavior, cap and message-vs-draft guidance satisfy the applicable items. |
| `gmail_draft_show` | draft_id, format, output_file, account | Existing draft ID example/discovery and closed format set retained; added output path example and inline omission behavior. |
| `drive_account_list` | none | Added `{}` example and YAML output guidance; clarified names apply to all Drive/Docs/Sheets tools. |
| `drive_auth_status` | account | Added account example and YAML output guidance; API verification distinction retained. |
| `drive_search` | query, limit, account | Added duplicate-grouping sibling guidance; existing Drive query example, limit default/cap and metadata output guidance retained. |
| `drive_dedupe` | query, limit, account | Replaced folder placeholder with an illustrative ID and search discovery guidance in field and tool text. Added individual-file search sibling guidance; cap retained. |
| `drive_file_read` | file_id, format, export_mime_type, output_file, verify, account | Added ID, MIME and path examples, ID source, verify omission behavior, tool example and structural Docs/selected-cell Sheets siblings. Existing format set and export defaults retained. |
| `drive_docs_info` | document_id, account | Added tool input example; existing URL ID example/source and full-read sibling retained. |
| `drive_docs_read` | document_id, tab, suggestions_view, output_file, account | Added tab ID example/source, path example/omission and tool example; existing suggestion set/default and structural-vs-export sibling retained. |
| `drive_sheets_info` | spreadsheet_id, account | Added ID example to rustdoc and explicit schema text and a tool input example; cell-read sibling retained. |
| `drive_sheets_read` | spreadsheet_id, range, sheet, render, output_file, account | Added ID example to both descriptions, title example/source/omission, output path example/omission, A1 tool example and info sibling. Existing range, render set/default and workbook cap retained. |
| `drive_docs_replace` | document_id, search, replace, ignore_case, dry_run, lease, account | Added ID discovery source and explicit lease omission/validation guidance. Existing literal find/replacement examples, empty-deletes behavior, boolean defaults, append sibling and write affordances retained. |
| `drive_docs_append` | document_id, text, text_path, dry_run, lease, account | Added ID source, file example/conditional requirement and lease omission/validation guidance. Existing text example, replace sibling and write affordances retained. |
| `drive_sheets_write` | spreadsheet_id, range, sheet, values, values_path, values_format, input, dry_run, lease, account | Added ID/title discovery, target requirement/omission, file example/conditional requirement, explicit file-format dependency and lease guidance. Existing row-major example, format/input sets/defaults and append/clear siblings retained. |
| `drive_sheets_append` | spreadsheet_id, range, sheet, values, values_path, values_format, input, dry_run, lease, account | Uses the same parameter struct as write; all changes above apply. Existing overwrite sibling, example and append retry warning retained. |
| `drive_sheets_clear` | spreadsheet_id, range, sheet, dry_run, lease, account | Added ID/title discovery, target requirement/omission and lease guidance; existing overwrite sibling, example and write affordances retained. |
| `drive_lease_acquire` | file_id, expiry_minutes, account | Added ID source and whole-minute example/omission behavior. Existing prompt, backup, existing-token, timeout and tagged-outcome guidance retained. |

## Applicability and deliberate exceptions

- IDs and paths are illustrative values, not existing resources. Callers must copy
  actual IDs from the named response fields or URL segments and select paths within
  `mcp.allowed_paths`. Message and thread IDs can look alike; the response field and
  resource type distinguish them. Draft IDs use their own namespace.
- Lease tokens are opaque and identify a real acquisition for one file/account.
  Giving a fabricated example would encourage an invalid write; the description
  instead directs callers to copy the acquired token. Dry-run bypasses the lease
  check; on real writes a supplied token is checked even when a rule permits omission.
- Boolean and closed-set values already provide concrete examples (`false`, `raw`,
  `formatted`, etc.); numeric defaults/caps likewise supply representative values.
  A separate `e.g.` label would duplicate them.
- Parameterless account-list tools use `{}` as their call example. The account
  macros give `work`, discovery tools, ambient/default/sole-account resolution and
  missing-default errors for every account-bearing tool. Added the existing complete
  process-environment credential bypass so explicit account selection is not overstated.
- Mutation/lease affordances do not apply to read-only tools; sibling guidance
  applies where operations overlap or ID types can be confused. Status, account
  discovery and label listing need no invented sibling.
- Required fields are identified in prose and generated schemas; paired optional
  inputs state their conditional requirements. Existing closed sets and defaults
  were checked against parsers, not inferred from JSON-schema types.
- All tool descriptions retain their CLI mapping. No purpose or mapping changed,
  so reverse CLI references, help snapshots and service guides need no updates.
  No nested-schema audit, runtime change, Markdown cleanup or prose-quality
  enforcement was introduced.
