# Claude AI Assistant Guide

gwi (Google Workspace Interface) is a Rust CLI and MCP server for Gmail, Drive, Docs, Sheets
and Slides. It is being extracted from [omni-dev](https://github.com/rust-works/omni-dev)
(rust-works/omni-dev#2203), so the code, conventions and tooling deliberately mirror it.

## Status

Phase 0 of the extraction: the repository, CI and name reservation only. The shared-code
strategy (Phase 1) is undecided, so do not copy modules from omni-dev ad hoc.

## Conventions

- [docs/STYLE_GUIDE.md](docs/STYLE_GUIDE.md) is **seeded verbatim from omni-dev** and still
  contains rules and examples about omni-dev subsystems (Atlassian, Datadog, the daemon).
  Prune it as the code it describes does or does not arrive. Use the tag lookup at the top.
- Commits use conventional format, enforced by `commit-lint.yml`. Scopes live in
  `.omni-dev/scopes.yaml`; check locally with `omni-dev git commit message lint origin/main..HEAD`.
- Before a commit run `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`
  and `cargo test`.
- `main` is protected and merges with a **merge commit** through a merge queue (set by the
  queue's ruleset; rebase merging stays enabled on the repo but the queue does not use it).
  Open a pull request and run `gh pr merge`, never `--admin`. If a non-required check is
  red `gh pr merge` falls back to auto-merge, which is disabled here; enqueue with the
  merge queue button, or the `enqueuePullRequest` GraphQL mutation.
- Merge commits were chosen because GitHub refuses to rebase-merge a pull request of a few
  hundred commits (`rebaseable: false` although `mergeable: true`), which the omni-dev
  history import (591 commits) hit. Squash merging stays off: it discards history.
- Add changelog bullets under `[Unreleased]` only.
