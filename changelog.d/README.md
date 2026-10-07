# Changelog fragments

`CHANGELOG.md` used to be the one file every PR edited, so any two PRs in flight
conflicted on the same hunk (#9), which the merge queue and concurrent worktree
sessions make worse. Instead, **each PR adds its own file here**;
`scripts/changelog.py collect` assembles them into `CHANGELOG.md` at release time.
Two PRs that each add a fragment never touch the same file.

The design and script come from succinctly
([rust-works/succinctly#3920](https://github.com/rust-works/succinctly/issues/3920));
omni-dev has its own variant. Whether to share one copy is a Phase 1 question of the
extraction (rust-works/omni-dev#2203), so this repository keeps its own.

## Adding a fragment

```
changelog.d/<id>.<type>[.<n>].md
```

| Part     | Meaning                                                                                 |
|----------|-----------------------------------------------------------------------------------------|
| `<id>`   | the issue number (`148`), or `+<slug>` (`+fix-typo`) for a PR with no issue             |
| `<type>` | `added`, `changed`, `deprecated`, `removed`, `fixed` or `security`                      |
| `<n>`    | optional positive integer for a second entry of the same id and type (`148.fixed.2.md`) |

The file body is the entry exactly as it is written in `CHANGELOG.md`: a bullet
(`- `), the prose, continuation lines indented two spaces. No section heading (the
type is the section) and no blank lines.

```markdown
- `gwi gmail search` accepts `--max-results` (#148). Defaults to 50; the server caps it
  at 500.
```

A follow-up that corrects or refreshes an entry edits **that PR's own fragment**
(if it has not been released yet), so it conflicts with nothing.

Anything in this directory other than `README.md` that is not a valid fragment name
fails the check, so a typo (`148.chnaged.md`) cannot silently drop an entry at release.

## What CI checks

The `Changelog fragments` workflow runs `python3 scripts/changelog.py check` (every
fragment's name, type and body) and, on a pull request, `check-pr` (the PR adds, edits
or renames a fragment). A PR with no user-visible effect (docs-only, CI-only, a
refactor) waives the second check with **either** the `no-changelog` label **or** a
`[no changelog]` line in the PR body (the marker is matched anywhere in the body outside
HTML comments, so do not quote it when merely describing it). Release PRs
(`chore(release): ...` that rewrite `CHANGELOG.md`) and bot PRs are waived automatically.

The workflow also runs on the merge queue's builds so that it always reports a status;
the fragment requirement is skipped there because it is a property of the PR.

## Releasing

```bash
python3 scripts/changelog.py collect --version X.Y.Z --dry-run   # preview the fragments
python3 scripts/changelog.py collect --version X.Y.Z             # rewrite CHANGELOG.md
```

`collect` renders `## [X.Y.Z] - DATE` with sections in Keep a Changelog order (Added,
Changed, Deprecated, Removed, Fixed, Security), entries by ascending issue number
(`+slug` entries last), updates the `[Unreleased]` / `[X.Y.Z]` compare links if
`CHANGELOG.md` has them, and deletes the consumed fragments. See
[docs/RELEASE.md](../docs/RELEASE.md).

## The existing `[Unreleased]` text

`CHANGELOG.md`'s `[Unreleased]` section predates fragments and is left untouched. At the
first release, `collect` carries whatever is under `## [Unreleased]` into the new section
verbatim, **after** the fragment sections, and leaves an empty `[Unreleased]` behind. From
then on the changelog is purely fragments. That first section therefore repeats `###`
headings (the fragments' sections, then the existing text's own). Nobody should add new
entries under `[Unreleased]` by hand.
