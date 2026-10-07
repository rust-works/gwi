# Contributing to gwi

Thanks for your interest. gwi is in its early extraction phase, so the best way to help right
now is to comment on the design in
[rust-works/omni-dev#2203](https://github.com/rust-works/omni-dev/issues/2203).

## Workflow

1. Fork and branch from `main`.
2. Keep changes focused. Run `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`
   and `cargo test`.
3. Use [conventional commits](https://www.conventionalcommits.org/); see
   [.omni-dev/commit-guidelines.md](.omni-dev/commit-guidelines.md) and
   [docs/STYLE_GUIDE.md](docs/STYLE_GUIDE.md).
4. Add a changelog fragment, `changelog.d/<issue>.<type>.md`, instead of editing
   [CHANGELOG.md](CHANGELOG.md) (see [changelog.d/README.md](changelog.d/README.md)). A change
   with no user-visible effect puts `[no changelog]` in the PR body instead.
5. Open a pull request. It merges with a merge commit through a merge queue.
