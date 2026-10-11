# Sheets MCP description coverage delta

Investigation of [#207](https://github.com/rust-works/gwi/issues/207), the
[coverage report on PR #198](https://github.com/rust-works/gwi/pull/198#issuecomment-6092531601).

## Archived comparison

The reported drop is reproducible from the original CI artifacts. It adds eleven
zero-hit executable lines attributed to the `drive_sheets_read` tool attribute,
without changing hit counts for handwritten behavior. The extra function is a
macro-generated schema-validation panic closure, not a cell-reading path or
execution of the description text.

The compared revisions are:

- Base: `19bab9447fc0a24749282999e8dd53c8a661897a`.
- Head: `90e27241a2138fb774750b9f98e0d8c10430d151`.

Download the head report from [CI run 38015366749](https://github.com/rust-works/gwi/actions/runs/38015366749)
and the baseline from [CI run 38011303273](https://github.com/rust-works/gwi/actions/runs/38011303273):

```bash
gh run download 38015366749 --repo rust-works/gwi -n coverage-summary -D /tmp/sheets-head
gh run download 38011303273 --repo rust-works/gwi -n coverage-baseline -D /tmp/sheets-base
patchcov -C "$WT" diff \
  --report /tmp/sheets-head/coverage-head.lcov \
  --baseline-report /tmp/sheets-base/coverage-head.lcov \
  --base-ref 19bab9447fc0a24749282999e8dd53c8a661897a \
  --head-ref 90e27241a2138fb774750b9f98e0d8c10430d151 \
  --strip-prefix /home/runner/work/gwi/gwi -o json
```

`WT` is a checkout with both revisions in its history. Explicit `--head-ref` is
necessary when replaying from a newer checkout: coverage markers must come from
the measured revision. The baseline is the exact merge-base report, not an
ancestor fallback: the head job downloaded it and skipped recomputation.

The original action was `action-works/patchcov-action` at
`65d82577d2fc3345dbda6829c3b1b76d5bb3ce3d`, with patchcov 0.1.1,
cargo-llvm-cov 0.9.1 and rustc 1.99.0 (`b940084d7`, 2026-09-28), on Linux.
Replay with local patchcov 0.4.0 reproduces the displayed figures:

| Measurement | Base | Head |
| --- | --- | --- |
| Raw executable lines (`LF`) | 279 | 290 |
| Raw covered lines (`LH`) | 265 | 265 |
| Lines removed by existing handler-glue markers | 14 | 14 |
| Executable lines after markers | 265 | 276 |
| Covered lines after markers | 265 | 265 |
| File percentage after markers | 100% | 95.72% |

Thus `100 × 265 / 276 = 95.7246…%`: the eleven extra zero-hit lines account for
all of the 4.28 percentage points. Overall coverage replays as 99.15% → 99.14%.
Patch coverage replays as 0/1; the single added executable line is head line 132.
The other ten zero-hit lines are unchanged attribute/description lines, not ten
new behavioral paths. The Coverage job passes because the action defaults to a
30% overall line gate and no patch gate; it does not require every file delta
to be nonnegative.

## Specific source and generated code

In the head's `src/mcp/drive_sheets_tools.rs`, lines **122–132** are the extra
`DA:<line>,0` records. Line 122 starts `#[tool(`; lines 123–132 lie inside the
multiline description. Line 132 adds the input example. The remaining description
lines 133–136 are not extra executable lines in this report.

The existing zero-hit handler glue remains separate:

- Base: 104–108 and 134–141, 143.
- Head: 113–117 and 144–151, 153.

Those fourteen lines are already ignored with a specific OAuth-fixture rationale.
Changing that exclusion would not address the new description-attributed region.

The head lcov has two instantiations of a zero-hit function whose mangled name
ends in `GwiServer27drive_sheets_read_tool_attr0Bb_`, both beginning at line 122.
The baseline has neither function record. Their overlapping regions contribute
eleven source lines to the denominator, not twenty-two. All 42 function records
shared by the reports have identical `FNDA` counts; none were removed, and the only additions
are these two zero-hit closure instantiations. Both reports still have the read
handler's async body with one hit and the fixture-backed `run_sheets_read` with
four hits. The eleven extra lines are therefore a change in emitted coverage
mapping for generated code, not a loss of hits in the read implementation.

Both revisions lock `rmcp` and `rmcp-macros` to **3.5.1**; PR #198 does not change
`Cargo.lock`. In that macro version, `tool.rs` constructs the input schema as:

```rust
rmcp::handler::server::common::schema_for_input::<Parameters<DriveSheetsReadParams>>()
    .unwrap_or_else(|e| {
        panic!(
            "Invalid input schema for `{}`: {}",
            std::any::type_name::<Parameters<DriveSheetsReadParams>>(),
            e
        )
    })
```

This expression belongs to the generated `drive_sheets_read_tool_attr` metadata
function. The closure is the schema-error fallback. `schema_for_input` validates
that the schema root has `type: "object"`. `DriveSheetsReadParams` derives an
object schema in both revisions. Its field descriptions cannot turn it into an
invalid root type. Successful tool registration leaves this panic fallback
unexecuted by design.

## Treatment

Keep the measurement and its existing exclusions. No broad macro/description
ignore, tolerated delta, handler refactor or deliberately invalid production
schema is needed. The head report validly records an unexecuted generated panic
fallback; the baseline's 100% figure did not include that fallback. The prose
change changes where/how coverage is attributed, rather than removing behavioral
tests. A coverage percentage is a ratio over the executable lines emitted in
that build; it is not a stable inventory of handwritten paths.

The tools/list inventory and nonblank-description checks exercise successful
registration. Existing wiremock tests exercise the read implementation and its
render/output behavior. Existing isolated handler tests exercise credential
failure. These tests should continue to pass without acquiring credentials,
changing tool descriptions, or weakening fixture isolation.

[#168](https://github.com/rust-works/gwi/issues/168) concerns lost child-process
profiles. It does not explain these newly emitted, zero-hit schema-error closures:
the same fallback is unused during successful registration in either process.

## Fresh measurement

To inspect the expanded metadata with the same macro version:

```bash
RUSTC_BOOTSTRAP=1 cargo rustc --manifest-path "$WT/Cargo.toml" \
  --lib --features mcp -- -Zunpretty=expanded > /tmp/sheets-expanded.rs
```

This is an inspection command only; `RUSTC_BOOTSTRAP` is scoped to that invocation.
The expansion confirms that the metadata function has exactly one closure, the
schema-validation panic fallback shown above.

On 2026-10-11, a fresh run at `a7767c6` (this investigation's starting base)
with rustc 1.99.0 and cargo-llvm-cov 0.9.1 on macOS arm64 passes the
all-features/workspace instrumented tests, including 5,357 unit tests and the MCP
integration suite:

```bash
cargo llvm-cov --manifest-path "$WT/Cargo.toml" --all-features --workspace \
  --lcov --output-path /tmp/sheets-current.lcov
cargo llvm-cov report --manifest-path "$WT/Cargo.toml" \
  --json --output-path /tmp/sheets-current.json
```

The fresh Sheets file has the same raw `LF:290`, `LH:265` counts and the same
95.72% after the fourteen handler-glue lines are removed. The read description
has moved to lines 136–150; its eleven zero-hit lines are **136–146**. Both closure
instantiations in LLVM JSON have `count: 0` and exactly one code region:

```text
[start_line, start_column, end_line, end_column, count, file_id, expanded_file_id, kind]
[136,        5,            146,      98,         0,     0,       0,                0]
```

That is direct region-level evidence tying all eleven lcov lines to the
schema-error closure. The macOS rerun reproduces the archived Linux head's
footprint despite later description attributes shifting its line numbers.
An initial filesystem-sandbox run denied fixture listeners (`PermissionDenied`);
the successful measurement allowed loopback for wiremock. No fixtures were changed.

A second run with fresh profiles also passes CI's exact instrumentation/test/
lcov-export sequence, including the doctest. It again emits `LF:290`, `LH:265`
and the two zero-hit read metadata closures:

```bash
(
  cd "$WT"
  export CARGO_TARGET_DIR="$WT/target/llvm-cov-target"
  eval "$(cargo llvm-cov show-env --sh)"
  cargo llvm-cov clean --profraw-only
  cargo test --manifest-path "$WT/Cargo.toml" --all-features --workspace
  cargo llvm-cov report --lcov --output-path /tmp/sheets-ci.lcov
)
```

The first driver run started with a clean workspace coverage build. This second
run resets raw profiles while reusing that build; it does not reuse the first
run's execution counts. For a cold reproduction, replace `--profraw-only` with
`--workspace`, as the action does before its test run.

This earlier investigation replayed historical CI reports and freshly measured
the later checkout; it did not rebuild both historical revisions. The archived
artifacts contain lcov and codecov summaries, not raw LLVM regions. They identify
which generated function appeared, but do not establish the compiler-internal
span/optimization decision that omitted it in the baseline. The local JSON and
macro expansion resolve the nature of the uncovered region without claiming that
internal decision had been diagnosed at that stage. The follow-up below tests
the compiler-span mechanism directly.

## Clean historical rebuilds (#287)

On 2026-10-11, separate clean builds of both historical revisions reproduce the
archived difference. Both complete instrumented test suites pass, including
5,281 library tests per revision and the integration suites and doctest. The
macro expansions retain the identical read-schema error expression in both.

| Measurement | Clean base | Clean head |
| --- | --- | --- |
| Raw executable lines (`LF`) | 279 | 290 |
| Raw covered lines (`LH`) | 265 | 265 |
| Read-schema fallback records in LLVM JSON | 0 | 2 |
| Fallback execution count | No mapping | 0 for both |
| Fallback code region | No mapping | `[122, 5, 132, 98, 0, 0, 0, 0]` for both |

All 42 shared `FNDA` records have unchanged hit counts, after matching the
lib/test crate disambiguators between the two worktree paths. The only two added
records are the read metadata fallback; none are removed. The two overlapping
regions still add eleven unique source lines. The
[evidence snapshot](repro-schema-coverage/results.json) records the raw regions,
function names, comparison result, compiler identity and artifact SHA-256s.

The successful builds used rustc 1.99.0 (`b940084d7`), LLVM 23.1.1,
cargo-llvm-cov 0.9.1, Linux **arm64**, Debian 13.7 in a Docker container, Linuxkit
7.0.12, uid 0 and `CARGO_BUILD_JOBS=4`. Both revisions used their committed
lockfiles, separate targets and fresh profiles. Permission-denial tests take
their existing root skip; no test, credential fixture or exclusion was changed.
The original head job log identifies Ubuntu 24.04 image `20261004.327.1` and
Linux **x86_64**. This is a clean historical mapping reproduction, not an exact
replica of that runner environment.

The initial emulated x86_64 baseline attempt passed 5,278 library tests but
failed three unrelated fixtures: missing-program detection in browser launch,
missing bare secret-command detection, and reuse within a short cache TTL.
It stopped before report export. An accidental restart overwrote its first full
log; the captured failure diagnostics are retained, but no historical x86_64
region comparison is claimed. The native arm64 runs pass those tests. The
standalone reproducer below separately succeeds on x86_64 with the same pinned
compiler, so the span mechanism does not depend on arm64. A complete historical
rebuild on the original Ubuntu x86_64 runner remains unverified.

To repeat the historical builds, create external worktrees for the base and
head SHAs listed above, install the pinned compiler, `llvm-tools-preview` and
cargo-llvm-cov, then run this sequence separately for each. `REPLAY_WT` is that
historical worktree and `REPLAY_OUT` is a fresh evidence directory. The recorded
run used `/work/base` and `/work/head` inside the Linux container.

```bash
mkdir -p "$REPLAY_OUT"
(
  set -euo pipefail
  cd "$REPLAY_WT"
  rustc -Vv
  cargo -V
  cargo llvm-cov --version
  uname -a
  export CARGO_BUILD_JOBS=4
  export CARGO_TARGET_DIR="$REPLAY_WT/target/arm64-coverage"
  eval "$(cargo llvm-cov show-env --sh)"
  cargo llvm-cov clean --workspace
  cargo test --manifest-path "$REPLAY_WT/Cargo.toml" --locked --all-features --workspace
  cargo llvm-cov report --manifest-path "$REPLAY_WT/Cargo.toml" \
    --lcov --output-path "$REPLAY_OUT/coverage.lcov"
  cargo llvm-cov clean --profraw-only
  cargo llvm-cov report --manifest-path "$REPLAY_WT/Cargo.toml" \
    --json --output-path "$REPLAY_OUT/coverage.json"
  unset RUSTFLAGS LLVM_PROFILE_FILE CARGO_ENCODED_RUSTFLAGS
  RUSTC_BOOTSTRAP=1 cargo rustc --manifest-path "$REPLAY_WT/Cargo.toml" \
    --lib --features mcp -- -Zunpretty=expanded > "$REPLAY_OUT/expanded.rs"
) > "$REPLAY_OUT/run.log" 2>&1
```

The subshell stops on a failed test or export, as CI does. Removing raw profiles happens only after lcov merges them; JSON reads the same
merged profile. The local evidence is retained under `/tmp/issue-287-evidence`,
with `base-arm64/` and `head-arm64/` reports, expansions and logs, the original
artifacts/job log, pinned coverage action, commands and x86_64 failure notes.
Only the compact snapshot and standalone source are committed; the full reports
contain machine-specific paths and can be regenerated with these commands.

## Reduced compiler-span reproducer (#287)

The attribution question in [#287](https://github.com/rust-works/gwi/issues/287)
has a description-only reproducer. The trigger is the **exclamation mark** in
`Sheet1!A1:C10`, not the new description's length, an invalid schema, or lost
execution profiles.

The standalone [fixture](repro-schema-coverage/Cargo.toml) retains the relevant
shape of rmcp's expansion: a generated metadata function whose identifier keeps
the handler's span, a description literal, and an `unwrap_or_else` panic closure
whose generated tokens use the attribute's call-site span. It needs neither rmcp
nor gwi. Its schema function always returns `Ok(true)` and its registration test
passes in both variants. The [driver](repro-schema-coverage/replay.py) copies it
into two independent workspaces and changes only `"x"` to `"x!"`.

```bash
rustup toolchain install 1.99.0 --component llvm-tools-preview
# Install cargo-llvm-cov 0.9.1, or use the pinned release binary.
cargo install cargo-llvm-cov --version 0.9.1 --locked
python3 "$WT/docs/repro-schema-coverage/replay.py" /tmp/schema-span-repro
```

Use a new output directory; the driver refuses to overwrite existing evidence.
`--toolchain stable` is equivalent only when `rustc +stable -Vv` identifies the
same compiler. Each variant has a clean coverage target and retains its source,
log, lcov, LLVM JSON, expansion, LLVM IR and before/after `InstrumentCoverage` MIR.
The driver uses CI's environment/clean/test/export sequence and fails unless the
plain variant has no mapped fallback and the bang variant has exactly one
zero-hit fallback region. No production schema or exclusion is changed.

### Why the mapping appears

In the pinned compiler's
[coverage span refinement](https://github.com/rust-lang/rust/blob/b940084d7eb6a299eb4bfeb8e34901bc051e7ac4/compiler/rustc_mir_transform/src/coverage/spans.rs),
`single_covspan_for_child_context` shortens a bang-macro expansion's call-site
span with `span_through_char(span, '!')`, intending to retain just a macro name
and its `!`. Here the generated `panic!` tokens have the **whole tool attribute**
as their call-site span. The search therefore reads the description text.

With no `!` in that attribute, the span stays equal to the generated closure's
body span. `extract_refined_covspans` explicitly discards spans that fill the
entire body. There is no usable span left, so the fallback has no coverage
mapping. With `Sheet1!A1:C10` present, the search stops just after that description's
`!`. This shortened span is inside the body and no longer equal to it, so it
survives. The pass adds a span at the start of the body, merges eligible spans,
and emits one region from the attribute start through that `!`.

This predicts the historical head's exact region: **122:5–132:98**. The `!` on
line 132 is column 97, and the region's end column is exclusive. Thus the mapping
covers eleven lines. The baseline attribute contains no `!`. The later checkout's
**136:5–146:98** region has the same explanation after the source lines move.

The [compact evidence snapshot](repro-schema-coverage/results.json) records the
reduced case on Linux x86_64 and macOS arm64 with rustc 1.99.0 and
cargo-llvm-cov 0.9.1. Both have no fallback mapping for `"x"`, and one zero-hit
region `[8, 1, 8, 27, 0, 0, 0, 0]` for `"x!"`.

The reduced case's closure exists in MIR and LLVM IR in both variants. Its
before-instrumentation control flow is unchanged; only the source literal and
span coordinates differ. The plain variant has no coverage mapping after the
instrumentation pass, while the bang variant already has its zero-hit region at
that stage. This distinguishes source-span refinement from removal of the
fallback's generated code, and places the change before LLVM/lcov export.

The justified next action is to offer this reproducer to Rust's coverage
maintainers: a bang-macro span originating in an attribute is shortened by
punctuation inside a string. No gwi runtime fix or coverage exclusion follows
from this diagnosis. The existing measurement remains a valid report of the
mappings emitted by each build.
