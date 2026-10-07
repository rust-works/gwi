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

### Changed

- Configuration lives in `~/.gwi/settings.json` and environment variables use the `GWI_`
  prefix (`GWI_PROFILE`, `GWI_GMAIL_ACCOUNT`, `GWI_CONFIG_DIR`, ...), with no fallback to
  omni-dev's `~/.omni-dev` and `OMNI_DEV_*` names. The unprefixed `GMAIL_*` credential
  variables keep their names. Moving existing accounts over needs the import command, which
  is not available yet.

## [0.0.1]

### Added

- Placeholder release reserving the `gwi` crate name. The Gmail and Drive functionality is
  being extracted from omni-dev (rust-works/omni-dev#2203).
