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

## Empty HOME required context

[Issue #179](https://github.com/rust-works/gwi/issues/179) added `Empty HOME Test`
from GitHub Actions (integration ID `15368`) to the same active ruleset on
10 October 2026 at 16:31:11 AEDT. This live settings change applies to ordinary
PRs targeting main and merge-queue entries immediately; merging this document
is not what enables it. The existing administrator bypass remains available.

The exact job name in [ci.yml](../.github/workflows/ci.yml) is unconditional and
reports on both `pull_request` and `merge_group`. It runs the
[runner regression tests](../scripts/test_home_write_test.py) and the
[empty-HOME guard](../scripts/home-write-test.py) for default and `mcp` features.
Keep the job name stable so required checks do not wait for a renamed context.

A fresh ruleset snapshot immediately preceded the rules-only update. Structural
comparison of the readback showed exactly one appended required check and the
server's changed timestamp. `Doc links`, `Commit Message Lint`, all queue
parameters, branch conditions, enforcement, and bypass actors were preserved.
Classic branch protection matched its original snapshot exactly. Effective
rules for `main` reported `Empty HOME Test` with integration ID `15368`.
The read-only policy commands above inspect these settings.

### Context compatibility evidence

- [PR #165's successful job](https://github.com/rust-works/gwi/actions/runs/38003919303/job/114068231124)
  reported `Empty HOME Test` for a pull request.
- [Successful merge-group job](https://github.com/rust-works/gwi/actions/runs/38025989795/job/114136835188)
  reported the same exact context from app `15368` on queue head
  `d272035a7b42fe8e90625676c1e658eadd9bad21`.

Both results predate this required-context update. They prove that both events
produce the configured context and that a passing queue result is compatible
with the requirement; they do not demonstrate post-update queue enforcement.
Inspect the queue job's provenance with:

```bash
gh api repos/rust-works/gwi/check-runs/114136835188
gh run view 38025989795 --repo rust-works/gwi --json event,headSha,jobs,conclusion,url
gh api repos/rust-works/gwi/rulesets/rule-suites
```

### Remaining live acceptance evidence

Direct evidence of a failed or missing `Empty HOME Test` preventing a merge,
and of a passing merge group satisfying the updated policy, is still needed.
Record the queue head, check result, and rule evaluation that attributes the
block or pass to this required context. A required-check readback alone does
not prove observed blocking. The issue-to-pr run does not enqueue or merge a
PR, modify another session's checks, or exercise administrator bypass to
manufacture this evidence.

## Hostile environment required context

[Issue #225](https://github.com/rust-works/gwi/issues/225) added
`Hostile Environment Test` from GitHub Actions (integration ID `15368`) to the
active main merge queue ruleset on 11 October 2026 at 00:21:18 AEDT, after
[PR #212](https://github.com/rust-works/gwi/pull/212) merged. This live settings
change applies immediately to ordinary PRs and merge groups targeting main;
merging this document does not enable it. The administrator bypass remains
available.

The unconditional job in [ci.yml](../.github/workflows/ci.yml) reports the exact
context on both `pull_request` and `merge_group`. It runs the
[runner regressions](../scripts/test_hostile_env_test.py) and the
[hostile-environment sweep](../scripts/hostile-env-test.py), covering the
MCP-enabled library and integration executables under valid inconvenient and
malformed settings. Keep the context name stable.

A fresh ruleset snapshot immediately preceded a rules-only update appending
`{"context":"Hostile Environment Test","integration_id":15368}`. Structural
comparison of the full readback showed only this addition and the server's
updated timestamp. Existing `Doc links`, `Commit Message Lint`, and
`Empty HOME Test` requirements, queue parameters, conditions, enforcement, and
bypass actors were preserved. Classic protection matched its pre-update snapshot
exactly. Effective rules for main reported the new context/app pair. The
read-only policy commands above inspect the current settings.

### Hostile environment context compatibility

- [PR #212's successful job](https://github.com/rust-works/gwi/actions/runs/38029719736/job/114147917468)
  reported the exact context from app `15368` on head
  `e6a842b384cc7f778abd467009b1dae370a24b97`.
- [Successful merge-group job](https://github.com/rust-works/gwi/actions/runs/38052057360/job/114213005316)
  reported the same context from app `15368` on queue head
  `b6e535ca129df99b79a39d860549dc934df5ddbe`; it completed on
  10 October 2026 at 23:33:52 AEDT.

Both jobs predate the policy update. The passing queue check matches the exact
required context/app combination, but does not demonstrate queue enforcement
under the updated policy. Inspect its provenance with:

```bash
gh api repos/rust-works/gwi/check-runs/114213005316
gh run view 38052057360 --repo rust-works/gwi --json event,headSha,jobs,conclusion,url
gh api repos/rust-works/gwi/rulesets/rule-suites
```

### Post-update healthy queue evidence

[Issue #255](https://github.com/rust-works/gwi/issues/255) verified the healthy
path after the requirement update. [PR #245](https://github.com/rust-works/gwi/pull/245)
merged on 11 October 2026 at 01:02:28 AEDT with queue head
`0b23280f6a9386b5c591700b042e8045bbf04c36` as its exact merge commit.
The [merge-group CI run](https://github.com/rust-works/gwi/actions/runs/38057232074)
reported [Hostile Environment Test](https://github.com/rust-works/gwi/actions/runs/38057232074/job/114228088742)
from app `15368` on that same head, completing successfully at 00:53:29 AEDT.
[Rule suite 4460201413](https://api.github.com/repos/rust-works/gwi/rulesets/rule-suites/4460201413)
reports `pass` for the active `required_status_checks` evaluation sourced from
ruleset `24584603`. These post-update check, evaluation and merge records
establish healthy queue acceptance under the updated policy.

Inspect the records with:

```bash
gh api repos/rust-works/gwi/check-runs/114228088742
gh run view 38057232074 --repo rust-works/gwi --json event,headSha,jobs,conclusion,url
gh api repos/rust-works/gwi/rulesets/rule-suites/4460201413
gh pr view 245 --repo rust-works/gwi --json mergedAt,mergeCommit
```

### Failed-check queue rejection

The authorized disposable [probe PR #259](https://github.com/rust-works/gwi/pull/259)
added only a first step to the existing hostile job that exits 1 when
`github.event_name == 'merge_group'`. Its normal PR required checks all passed
on head `986a84803584d0bd5e3e24cd8b3875170dfdbe54`. With the queue empty and the
exact hostile context/app requirement still active, the probe was enqueued
without bypass on 11 October 2026 at 12:13:03 AEDT.

The [merge-group CI run](https://github.com/rust-works/gwi/actions/runs/38101099486)
used queue head `a9040cd9a4f5c3ad245a75e84b28836f25e5d3f2`. Its
[Hostile Environment Test job and logs](https://github.com/rust-works/gwi/actions/runs/38101099486/job/114357022152)
report the exact context from app `15368`, with `failure` completed at
12:13:27 AEDT. The deliberate step exited 1 before checkout or the sweep.

The probe PR's `RemovedFromMergeQueueEvent` records removal at 12:13:52 AEDT,
reason `failed_checks`, and `beforeCommit` equal to that exact queue head.
The [API evidence snapshot](merge-queue-hostile-probe-255.json), captured after
rejection and before cleanup, records all 20 check runs: the hostile check is
the only failed check. Other checks were pending or successful; they are not
claimed to have all passed. The queue removal reason, same-head check results,
and sole failed required context establish rejection attributable to the
hostile requirement. No main update was attempted, so this rejection is
recorded by the queue timeline rather than a failed main-push rule suite.

Inspect the failure and queue removal with:

```bash
gh api repos/rust-works/gwi/check-runs/114357022152
gh api repos/rust-works/gwi/actions/jobs/114357022152/logs
gh api graphql -f query='{repository(owner:"rust-works",name:"gwi"){pullRequest(number:259){merged timelineItems(last:10,itemTypes:[REMOVED_FROM_MERGE_QUEUE_EVENT]){nodes{... on RemovedFromMergeQueueEvent{createdAt reason beforeCommit{oid}}}}}}}'
```

The probe was closed without merging, its remote branch deleted, and remaining
probe runs cancelled after evidence capture. The failed job retains its
`failure` conclusion; the overall CI run was subsequently cancelled for cleanup.
The queue was empty afterward. Full ruleset and classic-protection readbacks
matched the pre-probe snapshots exactly, including required checks, queue
parameters and administrator bypass. No other queue entries or sessions' checks
were modified. The deliberate workflow change is absent from the documentation
PR. This demonstrates the failed-check path; a separate missing-check probe was
not needed for #255's failed-or-missing acceptance criterion.

## Windows Clippy required context

[Issue #241](https://github.com/rust-works/gwi/issues/241) added `Windows Clippy`
from GitHub Actions (integration ID `15368`) to active ruleset `24584603` on
11 October 2026 at 00:23:08 AEDT. This live setting requires the check for ordinary
PRs targeting `main` and merge-queue entries immediately; merging this document
is not what enables it. The existing administrator bypass remains available.

The separate job in [ci.yml](../.github/workflows/ci.yml) retains the exact name
`Windows Clippy` and runs on `pull_request`, `merge_group`, and main pushes. It
uses native Windows with stable Clippy and runs both
`cargo clippy --all-targets -- -D warnings` and
`cargo clippy --all-targets --features mcp -- -D warnings`. The Linux `Clippy`
context is preserved. No new lint matrix or portability changes were needed.

### Configuration readback

A fresh ruleset read immediately preceded a rules-only update. Structural
comparison of readback showed exactly one appended context/app pair and the
server's changed update timestamp. All existing requirements (`Doc links`,
`Commit Message Lint`, `Empty HOME Test`, and `Hostile Environment Test`), branch
conditions, enforcement, bypass actors, and queue parameters matched. Classic
branch protection matched its pre-update snapshot exactly. Effective rules for
`main` reported `Windows Clippy` with integration ID `15368`. The read-only
policy commands above inspect the current configuration.

### Observed PR enforcement

Disposable [probe PR #244](https://github.com/rust-works/gwi/pull/244) targeted
`main`. Its first signed head,
`ee07c33533c9167a38c6751e1d25893c97097d8e`, used `[skip ci]` so no checks ran.
The PR merge panel explicitly displayed `Windows Clippy` as **Required**, with
“Expected — Waiting for status to be reported.” Normal merging waited for
requirements; the bypass checkbox remained unchecked.

The second signed head, `aecaed8c2ab768dadd2c18737f2d83137f40aa69`, replaced only
the probe branch's CI workflow with a native Windows job named `Windows Clippy`
that deliberately exited 1. Its
[failed Actions check](https://github.com/rust-works/gwi/actions/runs/38055589176/job/114223253789)
reported the exact name from app `15368`. The PR merge panel identified that
failed check as **Required**, and the API reported `mergeStateStatus: BLOCKED`.
Other required checks were also missing in these isolated probes, so this is
context-specific required-check UI evidence, not an experiment proving Windows
Clippy was the sole blocking condition. No merge or enqueue was attempted and
administrator bypass was never exercised.

The probe PR was closed without merging, its remaining run was cancelled, and
its remote branch was deleted. Probe workflow changes are absent from this PR.

### Merge-group compatibility and remaining evidence

A successful
[merge-group Windows Clippy job](https://github.com/rust-works/gwi/actions/runs/38052057360/job/114213005220)
reported `Windows Clippy` from app `15368` on queue head
`b6e535ca129df99b79a39d860549dc934df5ddbe`. Its CI run used the `merge_group`
event and succeeded. This exact context/app combination matches the new
requirement; the run predates the policy update and proves compatible reporting,
not observed post-update queue acceptance.

Direct post-update queue evidence remains outstanding: record a healthy queue
entry satisfying the new requirement and, if testing queue rejection, the queue
head and evaluation attributing rejection to failed or missing Windows Clippy.
The issue-to-pr workflow does not authorize enqueueing or merging a probe to
manufacture that evidence. Configuration readback, observed PR required-check
states, and historical merge-group compatibility are distinct evidence.
