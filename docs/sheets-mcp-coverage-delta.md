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

This is an exact replay of historical CI reports plus a fresh measurement of the
later checkout, not a fresh rebuild of both historical revisions. The archived
artifacts contain lcov and codecov summaries, not raw LLVM regions. They identify
which generated function appeared, but do not establish the compiler-internal
span/optimization decision that omitted it in the baseline. The local JSON and
macro expansion resolve the nature of the uncovered region without claiming that
internal decision has been diagnosed.
