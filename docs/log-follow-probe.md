# Investigating log-follow receive timeouts

[Issue #285](https://github.com/rust-works/gwi/issues/285) tracks initial-backlog
timeouts observed after the diagnostic and probe work in
[#185](https://github.com/rust-works/gwi/issues/185) and
[#243](https://github.com/rust-works/gwi/issues/243) was completed.
No production defect or need for a longer receive deadline has been established.
This opt-in tool runs the existing instrumented CLI recovery test, including its
immediate and split appends, without adding load to normal tests or CI.

## Run from a clean checkout

Requires Python 3.9 or later, Cargo and the normal build dependencies on Linux or
macOS. Each output directory must be new. Paths may be anywhere writable, preferably
outside the checkout; the output includes two complete Cargo build directories.

```sh
python3 scripts/log-follow-probe.py --output /tmp/gwi-follow-baseline --repetitions 3
python3 scripts/log-follow-probe.py --output /tmp/gwi-follow-startup \
  --repetitions 3 --cpu-workers 18 --workload startup --concurrency 4
python3 scripts/log-follow-probe.py --output /tmp/gwi-follow-harness \
  --repetitions 3 --cpu-workers 18 --workload harness --concurrency 1
```

Choose a CPU worker count appropriate for the host. The baseline adds no workload;
it does not assert the host is otherwise idle. `--features default` or `--features mcp`
selects one configuration; the default is both, measured sequentially. Builds finish
before any workload begins. Cargo JSON selects `cli_test` and its `gwi` CLI from
separate per-feature target directories, so another feature build cannot overwrite
the executable embedded in the test. No executable hash or temporary investigation
script is needed.

`startup` maintains the requested concurrent slots of repeated `gwi --help` processes.
`harness` maintains slots of the full CLI integration harness, excluding the separately
measured recovery test. It uses four libtest threads per slot by default; change this
with `--harness-threads`. Both workloads restart after exits, preserving each exit
status. CPU workers run Python integer arithmetic. Workers start before the first
focused invocation and stop after the last repetition of each feature configuration.
Ongoing harnesses are deliberately stopped; their partial activity is not a passing
full-suite verification. Completed harnesses must have a nonempty passing summary.

## Bounds and cleanup

The default total deadline is 2,400 seconds, including builds, with 900 seconds per
build, 60 seconds per focused repetition and 300 seconds per startup/harness command.
Override with `--max-seconds`, `--build-timeout`, `--run-timeout` and
`--workload-timeout`. These are supervision limits; the test's three ten-second
receive deadlines are unchanged. Repetitions, CPU workers, slots and harness threads
are explicitly configured; no automatic load tuning or failing-test retry occurs.

Every command has a separate POSIX session and writes directly to files, avoiding
pipe-drain waits on descendants. On completion, timeout, error, SIGINT or SIGTERM,
the runner signals the session's process group with TERM, allows up to 0.3 seconds,
then sends KILL to any remaining members and waits for the direct child. It also
checks group exit within a two-second observation window before advancing. This also
cleans inherited followers after a harness parent exits. Cleanup adds a bounded
allowance per active command beyond the total deadline. A macOS signal permission
denial triggers a `ps` group inspection (each call limited to five seconds):
a group already exiting is observed through the bounded exit window, and denial
is tolerated only after no live group member remains, and recorded in `cleanup_notes`. A denial
for a live member fails the probe; cleanup still attempts all other groups and
records the errors. Commands that deliberately
create another session escape this mechanism; the CLI fixtures do not do so.
SIGKILL of the runner itself cannot execute cleanup. Orphan zombies may briefly await
the system reaper but cannot run or retain output handles.

A failed or timed-out repetition fails the invocation even if later repetitions pass.
Workload failures, failed builds, missing exact test selections and missing phase
records fail it too. Exit status is zero only when all measured repetitions and
completed workloads pass; interruption returns 130. File output already written is
retained on failure and interruption.

## Evidence

`summary.json` records the revision, dirty-file list, platform, CPU count, configuration,
selected binary paths, repetition outcomes and durations, and all commands with PID,
UTC start time, timeout, outcome and return code. Each command directory contains
`command.json`, `stdout.log` and `stderr.log`. Build stdout is the Cargo JSON stream.
Focused stderr retains the fixture's full reader, child-state, cleanup and late-output
diagnostics. The summary copies raw events for:

- Initial backlog stdout.
- Recovered appended stdout.
- Malformed-line stderr.

Each successful repetition has six events: three for each append scenario. Event times
are relative to the fixture's follower spawn, not relative to the entire harness.
`first-focused` versus `subsequent-focused` describes invocation order within a feature
configuration; listing and workload startups may already have occurred. Filesystem
caches are neither cleared nor measured. Commands and dirty-file metadata make local
runs attributable, but retained evidence is local until explicitly shared.

Run the runner's regressions separately:

```sh
python3 scripts/test_log_follow_probe.py -v
```

These use fake Cargo/libtest executables and real process trees, including descendants
that inherit output handles and ignore TERM, parents that exit early, timeouts, and
SIGINT/SIGTERM of the actual runner.

The `Log Follow Probe Lifecycle` workflow runs this suite on Linux and macOS for
pull requests, merge queue entries and main pushes. Each matrix job has a
five-minute timeout and runs independently of the other platform. It exercises
fake Cargo/libtest and real process trees; actual CPU/full-harness comparisons
remain opt-in and do not run in ordinary CI.

## Linux smoke verification

Verification on 11 October 2026 (Australia/Sydney), Debian 13.7 in Docker,
`Linux-7.0.12-linuxkit-aarch64-with-glibc2.41`, Python 3.13.5 and Rust 1.99.0.
The container exposed 18 logical CPUs but was limited to four CPUs and 8 GiB RAM.
Source revision: `06ec9cbefe3e525c82fc36620678627a584e8129`.
Source and Git metadata were mounted read-only from the issue worktree; its
workflow/docs edits and transient Python bytecode are recorded in the dirty-file
metadata. No Rust or probe source changed for these measurements. Other worktree
builds were active on the host. No idle-host, cache or utilization control is claimed.

All 17 lifecycle regressions passed on this Linux environment (6.064 seconds)
and locally on macOS with Python 3.14.6 (46.601 seconds). No Linux-specific fix
or weakening of lifecycle assertions was needed.

Both invocations used fresh output directories, one repetition per feature,
and explicitly selected these bounds (seconds): total 1,800, each build 600,
each focused repetition 60 and each workload command 120. The loaded run
configured two CPU workers and one full-CLI-harness slot with four test threads.
These commands ran inside the container from the mounted worktree:

```sh
python3 scripts/log-follow-probe.py --output /evidence/baseline \
  --repetitions 1 --max-seconds 1800 --build-timeout 600 \
  --run-timeout 60 --workload-timeout 120
python3 scripts/log-follow-probe.py --output /evidence/harness \
  --repetitions 1 --max-seconds 1800 --build-timeout 600 \
  --run-timeout 60 --workload-timeout 120 \
  --cpu-workers 2 --workload harness --concurrency 1 --harness-threads 4
```

Both returned zero. All four first-focused repetitions passed their exact
test selection and retained six phase events each, three per append scenario.
Each invocation selected four distinct existing executable paths from Cargo
JSON under `/evidence/<comparison>/build/{default,mcp}`: one `gwi` and one
`cli_test` per feature, with no shared executable replacement.

Timing pairs below are **immediate / split append**, in seconds. Receive times
are relative to each follower spawn; wall time includes process cleanup.

| Workload | Features | Wall (s) | Initial backlog (s) | Recovered append (s) | Warning (s) |
| --- | --- | --- | --- | --- | --- |
| None added | default | 2.227 | 0.101 / 0.067 | 0.360 / 0.825 | 0.361 / 0.825 |
| None added | mcp | 2.002 | 0.114 / 0.046 | 0.370 / 0.554 | 0.370 / 0.554 |
| 2 CPU + 1 harness | default | 1.688 | 0.077 / 0.029 | 0.080 / 0.536 | 0.081 / 0.537 |
| 2 CPU + 1 harness | mcp | 1.934 | 0.075 / 0.035 | 0.342 / 0.549 | 0.347 / 0.550 |

Independent post-run `ps -axo pid=,pgid=,stat=,args=` snapshots were taken
inside the Linux container before it was stopped. Each recorded command PID
was audited as a process-group ID; zombie states were excluded from live
membership. Audits found no live members and every direct command had a
recorded return code after being waited for.

- Baseline: eight recorded groups, all commands passed.
- Loaded: fourteen recorded groups, eight metadata/build/listing/focused commands
  passed, four CPU workers stopped and two partial harnesses stopped.

Stopped harnesses are partial activity, not passing full-suite results.
No Linux invocation failed or was replaced by a retry. Raw summaries, command
logs, Cargo JSON, audit reports and process snapshots remain locally under
`/private/tmp/gwi-256-linux-evidence`, mapping to `/evidence` in the container.
The durable timing/status record is this guide and the
[issue evidence](https://github.com/rust-works/gwi/issues/256#issuecomment-6103986748).

This is one repetition per feature per workload on virtualized Linux; it
verifies artifact selection, phase capture and configured workload cleanup.
It does not establish sustained stress coverage, explain the historical timeout
or verify Windows behavior. Production behavior and receive deadlines are unchanged.

## macOS investigation

Measurements on 11 October 2026 (Australia/Sydney), macOS 26.5.2 arm64,
18 logical CPUs. The CLI/test source revision was `b6e535ca129df99b79a39d860549dc934df5ddbe`;
the runner and guide were under development, as recorded by each dirty-file list.
Each complete comparison ran three repetitions per feature, with both append scenarios
in each repetition. All 18 repetitions passed with six phase events apiece. No
historical receive timeout reproduced. Normal default/MCP suites and Clippy checks
also ran on this host during parts of the comparisons; other worktree builds were
observed. No host-idle, CPU-utilization or filesystem-cache control is claimed.

Raw evidence is retained locally in `/private/tmp/gwi-243-baseline`,
`/private/tmp/gwi-243-startup-fixed` and `/private/tmp/gwi-243-harness`. The exact
commands used the runner above with those output paths, `--repetitions 3`, and the
CPU/workload/concurrency arguments shown in the clean-checkout examples. Baseline
used an early runner spelling `--no-default-features`; this revision declares no
default features, so its default configuration is the same as ordinary `cargo test`.

Each timing pair below is **immediate / split append**, in seconds, read from the
labelled receive events. Wall time includes the entire focused process and cleanup;
phase times start at each scenario's follower spawn. Run 1 is first-focused, runs
2–3 are subsequent-focused. All rows have passing status.

| Workload | Features | Run | Wall (s) | Initial backlog (s) | Recovered append (s) | Warning (s) |
| --- | --- | --- | --- | --- | --- | --- |
| None added | default | 1 | 4.826 | 2.735 / 0.015 | 2.991 / 0.782 | 2.991 / 0.782 |
| None added | default | 2 | 1.881 | 0.016 / 0.033 | 0.272 / 0.535 | 0.272 / 0.535 |
| None added | default | 3 | 1.819 | 0.016 / 0.016 | 0.266 / 0.523 | 0.266 / 0.523 |
| None added | mcp | 1 | 4.507 | 2.678 / 0.016 | 2.933 / 0.525 | 2.934 / 0.525 |
| None added | mcp | 2 | 1.885 | 0.018 / 0.029 | 0.272 / 0.534 | 0.272 / 0.534 |
| None added | mcp | 3 | 1.873 | 0.015 / 0.017 | 0.270 / 0.523 | 0.270 / 0.523 |
| 18 CPU + 4 startup | default | 1 | 5.198 | 3.589 / 0.013 | 3.590 / 0.521 | 3.590 / 0.521 |
| 18 CPU + 4 startup | default | 2 | 1.886 | 0.013 / 0.014 | 0.268 / 0.592 | 0.268 / 0.592 |
| 18 CPU + 4 startup | default | 3 | 1.863 | 0.014 / 0.014 | 0.267 / 0.524 | 0.267 / 0.524 |
| 18 CPU + 4 startup | mcp | 1 | 5.181 | 3.557 / 0.017 | 3.558 / 0.527 | 3.558 / 0.527 |
| 18 CPU + 4 startup | mcp | 2 | 1.857 | 0.014 / 0.020 | 0.291 / 0.530 | 0.291 / 0.530 |
| 18 CPU + 4 startup | mcp | 3 | 1.877 | 0.013 / 0.014 | 0.268 / 0.525 | 0.268 / 0.525 |
| 18 CPU + 1 harness | default | 1 | 6.299 | 4.601 / 0.025 | 4.609 / 0.534 | 4.609 / 0.534 |
| 18 CPU + 1 harness | default | 2 | 1.965 | 0.022 / 0.021 | 0.276 / 0.530 | 0.276 / 0.531 |
| 18 CPU + 1 harness | default | 3 | 1.991 | 0.039 / 0.030 | 0.295 / 0.539 | 0.298 / 0.539 |
| 18 CPU + 1 harness | mcp | 1 | 8.379 | 6.167 / 0.020 | 6.422 / 0.784 | 6.422 / 0.784 |
| 18 CPU + 1 harness | mcp | 2 | 1.929 | 0.016 / 0.027 | 0.267 / 0.540 | 0.267 / 0.540 |
| 18 CPU + 1 harness | mcp | 3 | 1.950 | 0.022 / 0.015 | 0.274 / 0.524 | 0.274 / 0.524 |

The corrected startup comparison recorded 288 completed default startups and 280
completed MCP startups (568 total), with no startup failures. The harness comparison
completed two default harnesses and one MCP harness; a second MCP harness was
intentionally stopped after measurement. All completed harnesses passed. CPU workers
were stopped and direct children waited for at each feature boundary. A separate
post-run process-group audit found no live members for the completed comparisons.

An earlier startup invocation at `/private/tmp/gwi-243-startup` remains a **failed
invocation**, not a passing retry: its three default repetitions passed (4.969,
1.861 and 1.864 seconds), then macOS returned `PermissionError` during workload
cleanup; MCP measurement never started. An external audit found no surviving
recorded groups. Its final failure status was reconstructed in the evidence
summary because the original cleanup error interrupted the final summary write.
This was a runner lifecycle defect, not a log-follow receive timeout. The final
runner confirms group exit, distinguishes a permission denial for a live member
from one for an absent/exiting group, continues cleanup after an error, and retains
final failure status. Real and injected lifecycle regressions cover these paths.

The largest observed initial receive time in the complete comparisons was 6.167 seconds.
First-focused invocations were slower than subsequent invocations in these samples.
That observation does not identify loader, filesystem, reader scheduling or other
causes. The comparison covers a small number of CLI-harness overlaps, not repeated
full-library-suite overlaps or a controlled concurrent-build matrix. Linux runtime
execution and Windows behavior were not verified by this macOS investigation.
The historical timeout remains unexplained; no production behavior or receive
deadline changed.

If a receive timeout recurs, retain the failing directory without retrying it away.
Compare the receive timestamp with reader timestamps, late output and child state.
Late initial output motivates a spawn/readiness scheduling trace; timely initial output
followed by absent recovered output motivates a saved-offset/append handoff trace;
recovered stdout with missing warning motivates tracing stderr and malformed-line drain.
Choose the next trace from the actual failing phase rather than increasing deadlines.

## Post-instrumentation macOS comparison (#285)

[Issue #285](https://github.com/rust-works/gwi/issues/285) records three
post-instrumentation failures during unrelated PR verification: #271 / PR #281
(MCP), #208 / PR #284 (default), and #274 / PR #280 (MCP). Each initial receive
expired at about ten seconds, with the child still alive and both joined readers
reporting zero output after termination. No dependent append occurred. Passing
isolated and full-suite retries do not explain those failed invocations. They
supply evidence of absent initial output, not of an append-offset defect or a
specific startup/scanning/scheduling cause.

The following bounded comparison ran on 11 October 2026 (Australia/Sydney),
macOS 26.5.2 arm64, 18 logical CPUs, at source revision
`6fbe97454db07a0c97335ab59d272216b2cf9e66`. The existing probe and Rust source
were unchanged. Baseline and startup metadata record a clean checkout; harness
metadata records only the guide's opening-paragraph edit. Builds were isolated
per invocation and feature. First-focused still means invocation order, not a
cold cache. Host process snapshots recorded concurrent Cargo/rustc activity;
Clippy and full default/MCP verification also ran during parts of this comparison.
No host-idle, cache, CPU-utilization or controlled build-overlap claim is made.

Exact commands (the probe resolves this worktree's manifest itself):

```sh
WT=/Users/jky/wrk/work-trees/gwi/issue-285-log-follow-initial-timeout
python3 "$WT/scripts/log-follow-probe.py" --output /private/tmp/gwi-285-baseline \
  --repetitions 3 --max-seconds 2400 --build-timeout 900 \
  --run-timeout 60 --workload-timeout 300
python3 "$WT/scripts/log-follow-probe.py" --output /private/tmp/gwi-285-startup \
  --repetitions 3 --max-seconds 2400 --build-timeout 900 \
  --run-timeout 60 --workload-timeout 300 \
  --cpu-workers 18 --workload startup --concurrency 4
python3 "$WT/scripts/log-follow-probe.py" --output /private/tmp/gwi-285-harness \
  --repetitions 3 --max-seconds 2400 --build-timeout 900 \
  --run-timeout 60 --workload-timeout 300 \
  --cpu-workers 18 --workload harness --concurrency 1 --harness-threads 4
```

All three invocations returned zero. All 18 focused repetitions passed their
exact selection, record/warning assertions and six phase events each (108 events,
36 follower scenarios). No failed measurement was replaced by a retry. Each
invocation selected distinct default/MCP CLI and test executable paths through
Cargo JSON. The table uses receive timestamps relative to each follower spawn,
with **immediate / split append** pairs in seconds. Wall time covers the full
focused process and cleanup. Run 1 is first-focused; runs 2–3 are subsequent.

| Workload | Features | Run | Wall (s) | Initial backlog (s) | Recovered append (s) | Warning (s) |
| --- | --- | --- | --- | --- | --- | --- |
| None added | default | 1 | 4.362 | 2.529 / 0.014 | 2.783 / 0.521 | 2.783 / 0.521 |
| None added | default | 2 | 1.603 | 0.015 / 0.013 | 0.015 / 0.523 | 0.015 / 0.523 |
| None added | default | 3 | 1.559 | 0.014 / 0.014 | 0.014 / 0.521 | 0.014 / 0.521 |
| None added | mcp | 1 | 6.874 | 5.060 / 0.011 | 5.316 / 0.517 | 5.316 / 0.517 |
| None added | mcp | 2 | 1.808 | 0.012 / 0.011 | 0.263 / 0.520 | 0.263 / 0.520 |
| None added | mcp | 3 | 1.862 | 0.012 / 0.018 | 0.265 / 0.526 | 0.265 / 0.526 |
| 18 CPU + 4 startup | default | 1 | 4.944 | 3.076 / 0.017 | 3.329 / 0.532 | 3.329 / 0.532 |
| 18 CPU + 4 startup | default | 2 | 2.118 | 0.016 / 0.016 | 0.275 / 0.779 | 0.275 / 0.779 |
| 18 CPU + 4 startup | default | 3 | 1.847 | 0.017 / 0.015 | 0.275 / 0.530 | 0.275 / 0.530 |
| 18 CPU + 4 startup | mcp | 1 | 5.032 | 3.406 / 0.013 | 3.407 / 0.525 | 3.407 / 0.525 |
| 18 CPU + 4 startup | mcp | 2 | 1.856 | 0.015 / 0.018 | 0.274 / 0.536 | 0.274 / 0.536 |
| 18 CPU + 4 startup | mcp | 3 | 1.881 | 0.015 / 0.017 | 0.272 / 0.542 | 0.272 / 0.542 |
| 18 CPU + 1 harness | default | 1 | 5.084 | 3.510 / 0.013 | 3.512 / 0.521 | 3.512 / 0.521 |
| 18 CPU + 1 harness | default | 2 | 1.837 | 0.017 / 0.014 | 0.277 / 0.525 | 0.277 / 0.525 |
| 18 CPU + 1 harness | default | 3 | 1.875 | 0.015 / 0.014 | 0.267 / 0.532 | 0.267 / 0.532 |
| 18 CPU + 1 harness | mcp | 1 | 4.695 | 2.821 / 0.014 | 3.078 / 0.533 | 3.078 / 0.533 |
| 18 CPU + 1 harness | mcp | 2 | 1.883 | 0.015 / 0.013 | 0.270 / 0.524 | 0.270 / 0.524 |
| 18 CPU + 1 harness | mcp | 3 | 1.835 | 0.018 / 0.014 | 0.270 / 0.524 | 0.270 / 0.524 |

The largest initial receive was 5.060 seconds, in the first baseline MCP
invocation. First-focused receives were slower than subsequent receives in all
six feature/workload combinations. Added load did not monotonically increase
these timings. These observations do not identify the delay's cause or establish
that the ten-second historical stalls have disappeared.

Post-run `ps -axo pid=,pgid=,stat=,args=` snapshots were audited against every
recorded command PID as a process-group ID, excluding zombies from live members.
All direct commands had recorded return codes; all three audits found no live
members:

- Baseline: 12 groups; all commands passed.
- Startup: 624 groups; 576 startup commands passed, 36 CPU workers stopped,
  and all metadata/build/listing/focused commands passed.
- Harness: 55 groups; six complete background CLI harnesses passed, one partial
  harness stopped, 36 CPU workers stopped, and all other commands passed.

Stopped CPU workers and the partial harness are intentional cleanup, not test
passes. Four startup CPU commands and one harness CPU command recorded macOS
signal-denial cleanup notes followed by confirmation of no live group members.
There were no cleanup errors. The separate post-run audits confirmed the result.

Raw summaries, Cargo JSON, command logs, full child/reader/late-output diagnostics,
process snapshots and `audit.json` remain locally in the three output directories
above; build-context snapshots are `/private/tmp/gwi-285-build-processes.txt` and
`/private/tmp/gwi-285-mid-processes.txt`. Scratch evidence is temporary; the timings,
status, configuration, revision and limits recorded here are the durable result.

### Remaining uncertainty and next capture

No initial-backlog timeout reproduced within these bounds, so no failing child
stack or startup milestone was captured and no stage was attributed. This covers
three repetitions per feature/workload on one macOS host. It does not test a
controlled full-library-suite/concurrent-build matrix, establish a failure rate,
or add Linux/Windows runtime coverage. The earlier failures remain valid evidence;
this comparison does not replace them with successes. Production code, exact
assertions, ten-second receives and unconditional kill/wait/join cleanup are unchanged.

The next action is a bounded live-child capture during an initial-output stall,
before the fixture's cleanup removes the process. On macOS, a supervisor should
retain a process snapshot and a one-second `sample <follower-pid> 1` of that specific
live child, with an outer command timeout below the remaining receive deadline.
Record capture start/end, PID, revision, workload and the existing receive/reader
events. Sampling may perturb timing and needs its own bound; a failed or late
capture is missing evidence, not proof of a stage. Do not delay cleanup to obtain it.

Inspect that evidence before choosing further instrumentation: a stack before
`main` motivates loader/startup tracing; a stack in argument parsing or runtime
construction motivates startup milestones; scanning or stdout-writing stacks
motivate backlog/emission tracing. A live child in the follow loop with absent
reader events motivates stdout/reader scheduling capture. A single sample shows
where the child was during that interval, not necessarily where it spent the
whole receive deadline. If sampling cannot distinguish the stages, add opt-in
milestones at main entry, dispatch, scan start and first successful output write,
with a separate diagnostic sink so the exact stderr assertion remains intact.
No deadline or concurrency change is justified by this non-reproduction.
