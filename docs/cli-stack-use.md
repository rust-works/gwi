# CLI command-builder stack use

Issue #228 follows the Windows debug startup overflow fixed by the executable's
8 MiB reserve in `build.rs`. That reserve remains unchanged. It protects the
executable's main thread, but does not set the stack of a library caller's thread.

## Change and attribution

clap_derive 4.6.7's `Item::push_doc_comment`, `field_methods`, and
`final_top_level_methods` emit inferred documentation calls before explicit
`help`/`about` overrides. Each by-value builder call can keep another large
`Command` temporary in an unoptimized frame. Boxing command enum payloads does
not remove these builder temporaries.

The explicitly described variants of `DriveSubcommands` and `SheetsSubcommands`
now carry their original documentation in `#[doc = concat!("...")]` attributes.
The [Rust attribute expansion rules](https://doc.rust-lang.org/reference/attributes.html#meta-item-attribute-syntax)
expand macros in attributes after derives. The pinned clap extractor reads only
literal string expressions, so it skips these macro expressions. Rustdoc and the
compiler's missing-documentation lint receive the complete expanded text. The
explicit CLI descriptions, argument types, variant order, and dispatch remain
unchanged. Variants without complete explicit descriptions retain ordinary doc
comments. Twenty-nine variants explicitly retain `long_about = None`: their
inferred reset was necessary to clear a payload struct’s inherited long
description. The unchanged long-help snapshot identified these variants; simply
removing every inferred reset would change help output. This avoids a parser fork
or changing the public enum structure.

This is a targeted workaround for the pinned extractor, rather than a new clap
option. A future derive version could expand macro-valued documentation itself;
the help snapshots and stack probe must continue to pass when dependencies change.
A supported clap improvement that suppresses inferred calls when the corresponding
explicit method is present could eventually replace this workaround.

## Reproducing Windows measurements

On native x86_64 Windows with Rust stable, run from any directory:

```powershell
python "$WT/scripts/measure_cli_stack.py" --checkout "$WT" --output "$OUT/after-default"
python "$WT/scripts/measure_cli_stack.py" --checkout "$WT" --output "$OUT/after-mcp" --mcp
```

Use distinct output directories with the same parent to reuse dependencies. Each
measurement runs `cargo clean -p gwi` in that shared target directory: root-package
fingerprints must not reuse a baseline build whose worktree has newer source
mtimes than the candidate. Dependency artifacts remain cached. Source SHA-256
hashes, checkout paths, assembly paths, and Cargo artifact messages establish
provenance for each phase. The script uses the checkout's locked dependency versions and Cargo's dev profile
(`opt-level=0`). It records `rustc -Vv`, the checkout commit, feature selection,
MSVC unwind frame sizes, direct builder call paths, each probe's exit code, and
the smallest passing tested budget in `report.json`. `probe-<KiB>.log` retains
stdout/stderr even when a stack overflow aborts the child. A build failure or a
failure at 8 MiB fails the script.

`Windows CLI Stack` in CI runs this against the PR base and candidate with both
feature sets. The base receives the identical probe test and Cargo test target
registration; its production code remains unchanged. The job uploads four
reports, Cargo artifact messages, and the child logs as `windows-cli-stack`. The existing `Windows Build`
job continues to run real-binary help/version and log-follow tests in both feature
configurations, independently of this measurement job.

The probe builds and validates the entire command tree and update tree, parses
help/version, log search, a nested Sheets read, a nested update, and invalid
arguments. No commands execute or access credentials. Each requested stack budget
runs in a separate process so overflow cannot terminate the other measurements.
The sweep tests 512, 768, 1024, 1280, 1536, 1792, 2048, 3072, 4096, and 8192 KiB.
Its smallest passing budget is an empirical bound for these operations and this
compiler, not an exact high-water mark or a guarantee for arbitrary caller frames.

The static report sums `.seh_stackalloc` allocations and saved registers from
`.seh_pushreg`. Its direct call paths include only generated builder/parsing
functions present in the library assembly. Dependency frames, indirect calls,
return addresses, and caller overhead are excluded; tail jumps are conservatively
counted as calls. These paths explain frame attribution, but the subprocess sweep
establishes the tested cumulative startup budget.

## Local evidence

On aarch64 macOS, Rust 1.99.0 (`b940084d7`, 2026-09-28), default features,
dev profile, the emitted assembly's prologues give these byte counts:

| Generated builder | Before | After | Reduction |
| --- | ---: | ---: | ---: |
| Sheets normal and update, each | 548352 | 449392 | 98960 (18.0%) |
| Drive normal and update, each | 110768 | 99376 | 11392 (10.3%) |

The original long-help snapshot and a newly captured baseline short-help snapshot
cover every command. All 92 moved documentation blocks preserve their exact
original strings, including newlines and Markdown. Public and private rustdoc
builds check the expanded documentation and links.

Local cross compilation to Windows cannot build AWS-LC without Windows C headers.
Native Windows evidence is therefore collected by CI; macOS frame counts must not
be substituted for Windows measurements. Until the Windows reports establish a
smaller safe budget, the automatic Windows probe retains the known-safe 8 MiB
budget and the executable reserve stays at 8 MiB.
