# Releasing gwi

gwi has not had a release process beyond reserving the crate name (0.0.1), so this covers
only the changelog step. Add the version bump, tagging and publishing steps here when they
are settled.

## Update the changelog

Changelog entries are per-PR **fragments** in `changelog.d/` (see
[changelog.d/README.md](../changelog.d/README.md)), not edits to `CHANGELOG.md`;
`scripts/changelog.py` assembles them. Preview, then write the release section:

```bash
python3 scripts/changelog.py check                                # every fragment is well-formed
python3 scripts/changelog.py collect --version X.Y.Z --dry-run    # preview; writes nothing
python3 scripts/changelog.py collect --version X.Y.Z              # rewrite CHANGELOG.md
```

`collect` does the following, so there is nothing to move or reformat by hand:

- renders `## [X.Y.Z] - YYYY-MM-DD` with the sections in Keep a Changelog order (Added,
  Changed, Deprecated, Removed, Fixed, Security), entries by ascending issue number,
  `+slug` entries last;
- carries whatever is under `## [Unreleased]` into that section **verbatim, after the
  fragment sections**, and leaves an empty `## [Unreleased]` above it. The text that
  predates fragments is therefore released once, in the first release made after the switch;
  after that `[Unreleased]` stays empty and the flow is purely fragments. That first section
  repeats headings (a `### Added` from the fragments, then the existing text's own);
- updates the `[Unreleased]` and `[X.Y.Z]` comparison links at the bottom, if
  `CHANGELOG.md` has them (it has none today);
- deletes the consumed fragments (everything in `changelog.d/` except `README.md`).

It refuses, writing and deleting nothing, if a fragment is malformed, the version is not
`X.Y.Z`, the version already has a section, or there is nothing to release. Read the result
(`git diff CHANGELOG.md`) and group or reword entries by user impact if the release needs it.

Commit it as `chore(release): ...`; that title waives the "PR needs a fragment" check for a
PR that rewrites `CHANGELOG.md`, since a release deletes fragments rather than adding one:

```bash
git add CHANGELOG.md
git add -A changelog.d    # the consumed fragments are deleted
git commit -m "chore(release): prepare vX.Y.Z"
```
