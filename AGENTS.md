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
- `main` is protected and merges by rebase through a merge queue: open a pull request and
  run `gh pr merge`, never `--admin`.
- Add changelog bullets under `[Unreleased]` only.
