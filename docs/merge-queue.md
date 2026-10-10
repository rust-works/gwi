# Merge queue required lint

## Policy

The active [main merge queue ruleset](https://github.com/rust-works/gwi/rules/24584603)
requires `Commit Message Lint` from GitHub Actions (integration ID `15368`) on the
default branch. The job in
[commit-lint.yml](../.github/workflows/commit-lint.yml) runs commit message lint
over the merge group's exact `base_sha..head_sha` range and scopes lint against
`src`, `.github`, `tests`, and `scripts` in the combined queue tree.

[Issue #145](https://github.com/rust-works/gwi/issues/145) added this required
context on 10 October 2026 at 12:42:24 AEDT, after
[PR #144](https://github.com/rust-works/gwi/pull/144) added queue workflow coverage.
This was a live GitHub settings change; merging this document does not apply it.

The ruleset's existing `Doc links` requirement, conditions, administrator bypass,
and merge queue parameters were preserved. Classic branch protection was also
preserved: it requires `Test (stable)`, `Test (nightly)`, `Windows Build`,
`Rustfmt`, `Clippy`, `Docs`, and `Changelog fragments`. The new lint requirement
comes from the ruleset, without duplication in classic protection. Normal queue
merges must satisfy it; the existing administrator bypass remains available.

## Verification record

Before the update, the ruleset and classic protection were captured through the
GitHub API. A fresh ruleset read immediately preceded a rules-only update that
appended the new context. Structural comparison of the readback with that fresh
snapshot showed exactly one additional required check; all other fields matched
apart from the server's update timestamp. Classic protection matched its original
snapshot exactly. The effective rules for `main` also reported the new context
and integration ID from ruleset `24584603`.

The following read-only commands can inspect the current policy:

```bash
gh api repos/rust-works/gwi/rulesets/24584603
gh api repos/rust-works/gwi/rules/branches/main
gh api repos/rust-works/gwi/branches/main/protection
```

A real [merge-group Actions run](https://github.com/rust-works/gwi/actions/runs/38013416205)
reported the exact `Commit Message Lint` context from app `15368` on queue head
`426bf4fa6eecca0fc9f189e49d9842fb7d7f073d`. Both lint steps succeeded:

- Commit range: `19bab9447fc0a24749282999e8dd53c8a661897a..426bf4fa6eecca0fc9f189e49d9842fb7d7f073d`.
- Commit message lint: eight commits passed, zero with issues.
- Scopes lint: 13 scopes, 274 files checked, zero violations.

The job's result and logs can be inspected with:

```bash
gh run view 38013416205 --repo rust-works/gwi --json jobs,event,headSha,conclusion,url
gh run view 38013416205 --repo rust-works/gwi --log
gh api repos/rust-works/gwi/commits/426bf4fa6eecca0fc9f189e49d9842fb7d7f073d/check-runs
```

## Pending live acceptance checks

The successful run above predates the required-context update. It proves that the
queue workflow reports the correct context and runs both lints, but does not prove
queue enforcement under the updated policy. No post-update queue rejection or
healthy merge was available in the Actions and rule-suite evidence inspected for
this record.

Before treating #145 as fully verified, record direct evidence of both:

1. A queued group with a failed `Commit Message Lint` result being prevented from
   merging because of that required check.
2. A healthy queue entry passing the required context and proceeding normally
   under the updated policy.

Link the queue head SHA, Actions job, and queue/rule evaluation for each case.
Keep policy readback distinct from observed queue behavior. The issue-to-pr run
did not enqueue or merge a PR, inject failures into another session's checks, or
remove administrator bypass to manufacture this evidence.
