# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- CI coverage job using `action-works/patchcov-action`, which posts a coverage diff on pull
  requests and publishes the baseline from `main`.
- `gwi gmail`: search, read, thread, label, draft, sync, sync-all, extract-attachments,
  render and insert, with named accounts and OAuth2 login. Extracted from `omni-dev gmail`
  (rust-works/omni-dev#2203) with its history; the unreleased code is only available from
  source until the first Gmail release.
- A global `--profile` flag selecting a credential profile from `~/.gwi/settings.json`.
- `gwi-mcp`, an MCP server (built with the `mcp` feature) exposing the eight read-only
  Gmail tools (`gmail_auth_status`, `gmail_account_list`, `gmail_search`,
  `gmail_message_read`, `gmail_thread_read`, `gmail_label_list`, `gmail_draft_list`,
  `gmail_draft_show`) under the names omni-dev used. An optional `mcp` block in
  `~/.gwi/settings.json` sets `log_level` and `max_response_bytes`.
- `gwi import`: copies Gmail and Drive accounts, the lease settings, the Google environment
  variables, and the `mcp` log level and response cap from omni-dev's
  `~/.omni-dev/settings.json` into gwi's, so an existing omni-dev user does not have to log in
  again. It never modifies the source, never overwrites a different value without `--force`,
  is safe to run twice, supports `--dry-run` and never prints a secret. Drive's lease ledger
  is not copied yet.

### Changed

- Configuration lives in `~/.gwi/settings.json` and environment variables use the `GWI_`
  prefix (`GWI_PROFILE`, `GWI_GMAIL_ACCOUNT`, `GWI_CONFIG_DIR`, ...), with no fallback to
  omni-dev's `~/.omni-dev` and `OMNI_DEV_*` names. The unprefixed `GMAIL_*` credential
  variables keep their names. Use `gwi import` to move existing accounts over.

## [0.0.1]

### Added

- Placeholder release reserving the `gwi` crate name. The Gmail and Drive functionality is
  being extracted from omni-dev (rust-works/omni-dev#2203).
