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

## Native Windows evidence

[CI run 38059252135](https://github.com/rust-works/gwi/actions/runs/38059252135)
used x86_64 Windows MSVC, Rust 1.99.0 (`b940084d7`, 2026-09-28), dev profile,
baseline `b6e535ca129df99b79a39d860549dc934df5ddbe`, and PR merge tree
`f4cccd7259f72106b1b077a6beb22d48de89f4f9` (feature head `ede39a3`). Every
library artifact reported `fresh: false`; the recorded candidate builder-source
SHA-256 hashes match this checkout after conversion to Windows CRLF line endings.
The `windows-cli-stack` artifact contains four complete reports, Cargo messages,
and all probe logs. Both feature configurations produced identical numbers:

| Generated builder or path | Before bytes | After bytes | Reduction |
| --- | ---: | ---: | ---: |
| Sheets normal and update, each | 548328 | 449368 | 98960 (18.0%) |
| Drive normal and update, each | 110744 | 99352 | 11392 (10.3%) |
| Largest recorded direct builder path | 1017464 | 907112 | 110352 (10.8%) |

The largest path goes through `Cli::command_for_update`, the root argument and
subcommand builders, Drive, Sheets, `UpdateConditionalFormatCommand`, and
`ConditionalFormatRuleArgs`. Its unchanged leaf has a 230120-byte normal frame;
that type's update frame is 263944 bytes. The report retains every symbol and
frame in the path, including boxed argument forwarding and intermediate builders.

| Thread budget | Baseline default / MCP | Candidate default / MCP |
| --- | --- | --- |
| 512 / 768 KiB | Stack overflow | Stack overflow |
| 1024 KiB (1 MiB) | Stack overflow | Pass |
| 1280 / 1536 / 1792 / 2048 / 3072 / 4096 / 8192 KiB | Pass | Pass |

Overflow exits were Windows `0xC00000FD` (Python exit code 3221225725).
The smallest passing tested budget fell from 1280 to 1024 KiB. This demonstrates
construction and representative parsing, including update parsing, at the issue's
1 MiB target with both feature sets. The ordinary isolated regression test now
requires 1 MiB on Windows as well as other platforms. Windows Build also passed
the real binary help/version and log-follow tests in both feature configurations.

The executable reserve remains 8 MiB. The reduced direct path alone still occupies
about 0.87 MiB before dependency, caller, and runtime frames. The isolated library
probe does not bound the full Tokio/main-thread startup stack, so its success does
not justify lowering the executable reserve.

The earlier comparison in run 38057923150 is invalid: its shared target reused
baseline root artifacts for the older candidate checkout. A minimal two-checkout
regression reproduced Cargo's `fresh: true` result for different source contents;
forcing root cleanup rebuilds the correct candidate. The fresh-build run above
supersedes that comparison. Local cross compilation remains unavailable without
Windows C headers for AWS-LC; all Windows conclusions come from native CI.
