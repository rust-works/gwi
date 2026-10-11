# Disposable Empty HOME failed-result probe

This branch prepares a probe for [#179](https://github.com/rust-works/gwi/issues/179).
**Do not merge this PR.** The workflow modifications are disposable experiment
code, not a production change. No negative enforcement evidence is established
by preparing or opening it. Enqueue only after authorization for this concrete
experiment; queue entry can cause an automatic merge.

## Exact behavior and independent guard

The first step of `home-write-test` in [ci.yml](../.github/workflows/ci.yml)
exits 1 only for `merge_group`. Its context remains `Empty HOME Test`, emitted
by GitHub Actions app `15368`. Pull-request runs still execute the original
runner regressions and empty-HOME sweep.

The independently required `Docs` job has no dependency on Empty HOME. Its first
step, also only for `merge_group`, sleeps 900 seconds and then exits 1. The job
has a 20-minute timeout. It has no success path on a queue build: queued/pending,
failure, cancellation and timeout do not provide a successful required result.
If the target requirement is ineffective, the separate classic-protection
`Docs` requirement still prevents an ordinary merge. This guard assumes normal
GitHub required-check enforcement and does not protect against administrator
bypass or deliberate mutation of the guard result. Neither is part of the probe.

The delay allows capture of queue rejection while Empty HOME is the only failed
required check. It is not a race to close a PR before it merges. Normal PR Docs
steps remain present; their new job timeout is 20 minutes.

## Preflight before the separately authorized queue operation

1. Refresh the [full protection snapshots](merge-queue.md#verification-record),
   retaining raw responses and timestamps. Confirm active ruleset `24584603`
   requires `Empty HOME Test` from app `15368`, classic protection requires
   `Docs`, and all other production settings match the preparation snapshots.
   Do not overwrite changes made by concurrent sessions.
2. Verify the exact signed PR head and diff contain the two probe steps and
   timeout shown above. Confirm there is no job condition, dependency,
   `continue-on-error`, duplicate successful `Docs` check or other mechanism
   that could let the safeguard succeed on the queue head. Check every page of
   check runs and commit statuses when inspecting a head.
3. Require successful normal PR checks, including Empty HOME and Docs. The
   draft must be marked ready only for the authorized experiment, with its
   DO NOT MERGE warning retained. Do not enable auto-merge or use bypass.
4. Require an empty merge queue and verify no other session owns this probe.
   Abort rather than affecting another entry. Recheck protection and head
   immediately before enqueue. Authorization is for this owned PR/head, one
   ordinary enqueue and owned cleanup, not for any merge or policy change.

## Evidence capture and attribution

Retain the PR URL, PR head, queue head, queue entry timestamp and exact
`merge_group` Actions run. Capture the failed Empty HOME check's ID, app ID,
conclusion, timestamps and logs. Inspect all check-run and status pages on that
exact queue head, including the pending Docs guard.

Capture the PR's `RemovedFromMergeQueueEvent`: timestamp, `reason` and
`beforeCommit.oid`. For example, substituting the owned PR number:

```bash
gh api graphql -f query='{repository(owner:"rust-works",name:"gwi"){pullRequest(number:PROBE_NUMBER){merged mergeCommit{oid} timelineItems(last:100,itemTypes:[REMOVED_FROM_MERGE_QUEUE_EVENT]){nodes{... on RemovedFromMergeQueueEvent{createdAt reason beforeCommit{oid}}} pageInfo{hasPreviousPage startCursor}}}}}'
```

Require `failed_checks` removal on the exact queue head, with Empty HOME the
sole failed required result at rejection and the Docs guard still pending.
Other pending checks are not represented as passing. If another check fails,
the guard finishes first, the queue reports a different reason, or no attributed
removal is available, preserve the observations and leave acceptance unresolved.
A generic PR `BLOCKED` state alone is insufficient.

If no rejection appears before the guard fails, stop the experiment and clean
up. Do not rerun the safeguard to manufacture a success. Retain API evidence
before cancelling remaining owned runs; cleanup can change their conclusions.

## Cleanup and production PR

Close the probe without merging, cancel only its remaining runs, delete only
its remote branch and confirm it is absent from the queue. Record `merged:
false`, and verify that main's workflow lacks both probe steps. Compare fresh
full ruleset and classic-protection snapshots with the preflight snapshots.
Explain concurrent drift rather than restoring settings blindly.

Replace the disposable workflow modifications with an evidence-only production
PR after cleanup. Link the exact head, failed job/logs, queue removal, protection
comparison and cleanup. Do not merge this experiment PR or treat its closing
footer as evidence that #179 is resolved.

## Missing-context case remains separate

The production response window is 60 minutes. The 15-minute Docs guard makes
this probe unsuitable for missing-context verification: it fails before that
window ends, and even a longer pending Docs guard would make a check-response
timeout ambiguous. A renamed, skipped or cancelled Empty HOME check also does
not prove an absent expected result prevented merging.

Use naturally occurring context-specific evidence or prepare a separate guarded
method yielding an explicit refusal attributable to the missing Empty HOME
result. Check every check-run/status page on the exact head and preserve the
expected-context wait and eventual refusal. Do not claim both negative cases
from this failed-result experiment.
