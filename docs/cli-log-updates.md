# Updating the log parser

The public `Cli` parser supports partial updates of a search-only `log` command:

```rust
use clap::Parser;
use gwi::Cli;

let mut cli = Cli::try_parse_from(["gwi", "log", "--limit", "0"]).unwrap();
cli.try_update_from(["gwi", "log", "--limit", "1"]).unwrap();
```

The limit becomes 1. Unsupplied search filters, output format, boolean flags and
the global profile remain unchanged. Supplied repeatable flags follow clap's
update behavior: they replace the stored list.

An update can select `log prune`, and subsequent updates of `log prune` retain
unsupplied prune options. As with other clap partial updates, omitting a
subcommand retains an already selected action. Thus an update containing only
`log` does **not** switch an existing prune command back to search; use a fresh
`Cli::try_parse_from` to do that. Updates validate their input, not the combination
of previously stored and newly supplied fields. Avoid applying search options to
a stored prune command: they do not clear its action and are not used by prune.

Ordinary parsing and update input validation still reject search flags combined
with `prune`, including flags placed before the action. The global `--profile`
flag remains accepted in both modes.

## Minimal clap reproducer

The original failure can be reproduced without gwi or the conflict setting using
clap 4.6.7 with its derive feature:

```rust
use clap::{Parser, Subcommand};

#[derive(Parser)]
struct Log {
    #[command(subcommand)]
    action: Option<Action>,
    #[arg(long)]
    limit: Option<usize>,
}

#[derive(Subcommand)]
enum Action {
    Prune,
}

let mut log = Log::try_parse_from(["log", "--limit", "0"]).unwrap();
let error = log.try_update_from(["log", "--limit", "1"]).unwrap_err();
assert_eq!(error.kind(), clap::error::ErrorKind::MissingSubcommand);
```

In clap_derive 4.6.7's `src/derives/args.rs::gen_updater`, an optional-subcommand
field with no existing value unconditionally calls the action enum's
`from_arg_matches_mut`. The enum constructor requires a subcommand even when the
matches contain none. Initial parsing correctly checks whether an action is
present. Adding `args_conflicts_with_subcommands = true` does not cause or cure
the update failure.

gwi works around this in a private flattened argument adapter in
[src/cli/log.rs](../src/cli/log.rs). The adapter checks for a supplied subcommand
before constructing an action, delegates existing-action updates to the derived
prune payload, and delegates both command builders to that same enum. Search fields remain
derived. Before their updates, the adapter removes implicit default values for
search booleans and output, and prune booleans, so omitted options retain stored
values. The adapter is the first field because derived field updates run in
declaration order. Explicitly supplied values are left intact. This leaves help output, validation and prune option handling with clap,
without duplicating the entire parser or modifying the dependency.

## Default-valued update reproducer

This separate program avoids subcommands entirely. With
`clap = { version = "=4.6.7", features = ["derive"] }`, save it as `src/main.rs`
in a scratch Cargo binary and run `cargo run --locked` after generating its lockfile:

```rust
use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    Oneline,
    Json,
}

#[derive(Parser, Debug)]
struct Search {
    #[arg(long)]
    follow: bool,
    #[arg(long, default_value = "oneline")]
    output: String,
    #[arg(long, value_enum, default_value_t = Format::Oneline)]
    format: Format,
}

fn main() {
    let mut search = Search::try_parse_from([
        "search", "--follow", "--output", "json", "--format", "json",
    ]).unwrap();
    search.try_update_from(["search"]).unwrap();
    assert!(!search.follow);
    assert_eq!(search.output, "oneline");
    assert_eq!(search.format, Format::Oneline);
    println!("{search:?}");
}
```

The omitted boolean, string and enum options all overwrite stored values with
implicit defaults. This differs from gwi's retained-unsupplied-state convention.
An explicitly supplied `--output oneline` or `--format oneline` also selects the
default, but must remain an update: filtering must distinguish value source,
not compare the new value with the default.

## Upstream status and adapter decision

Checked on 11 October 2026 with rustc 1.99.0 (b940084d7 2026-09-28), on macOS.
The [latest upstream release](https://github.com/clap-rs/clap/releases/tag/v4.6.7)
and the [crates.io sparse index](https://index.crates.io/cl/ap/clap) both identified
4.6.7 as the latest stable release; its index entry was not yanked. gwi's
`Cargo.lock` already pins clap, clap_builder and clap_derive to that same version.
A standalone project pinned to `=4.6.7` resolved all three components to 4.6.7.
One exact-version run therefore covers both the locked dependency and current
supported release; there was no newer release to compare.

The standalone run used `cargo generate-lockfile --offline` followed by
`cargo run --locked --offline`, with the dependencies already cached. It checked
the optional-action reproducer both with and without the conflict attribute,
and the independent default-value parser above, plus explicit default output
updates after restoring nondefault values:

| Case                                             | clap / clap_derive | Observed result                              |
|--------------------------------------------------|-------------------|----------------------------------------------|
| Absent optional action, no action in update       | 4.6.7 / 4.6.7     | `MissingSubcommand`                           |
| Same case with `args_conflicts_with_subcommands`  | 4.6.7 / 4.6.7     | `MissingSubcommand`                           |
| Omitted boolean / string / enum defaults         | 4.6.7 / 4.6.7     | `true` / `json` / `Json` reset to defaults     |
| Explicit `oneline` string / enum output           | 4.6.7 / 4.6.7     | Explicit default output accepted              |

The optional-action failure is now tracked by
[clap #6550](https://github.com/clap-rs/clap/issues/6550). Before filing, issue
searches covered `MissingSubcommand`, `try_update_from`, `update_from subcommand`
and `update "optional subcommand"`, including closed reports; discussion searches
covered `try_update_from` and `update_from subcommand`. No matching report was
found. [#4617](https://github.com/clap-rs/clap/issues/4617) concerns explicitly
required arguments and does not establish a contract for this optional case.
The new report asks whether the optional-action construction is intentional;
no upstream contract decision or released fix was available at this check.

Default-valued updates continue to use
[clap #4501](https://github.com/clap-rs/clap/issues/4501). The maintainer suggested
[ignoring default-sourced matches](https://github.com/clap-rs/clap/issues/4501#issuecomment-1323747059),
but the [compatibility classification remains unresolved](https://github.com/clap-rs/clap/issues/4501#issuecomment-1597464175).
The standalone results show the same default-source mechanism affects gwi's
boolean and enum-valued output cases, and the optional-action report notes this
connection. This is our observed relationship to #4501, not an upstream promise
that enum output has a supported retention contract. Neither case has a verified
released resolution.

**Decision:** retain the existing clap dependency and both responsibilities of
`LogActions`. Version 4.6.7 cannot replace either the optional-action guard or the
default-source filtering. Keep the adapter first in `LogCommand`, and keep
filtering prune's implicit booleans before its derived payload updater. Replacing the adapter with derived parsing after only an optional-action fix
would still reset search/prune flags and output; a default-value fix alone
would still reject search-only updates.
No parser or lockfile change is warranted by these results.

Revisit this decision when a supported release resolves either case. First run
both standalone reproducers against the candidate with exact component versions.
Then try removing only the responsibility it resolves, and run the existing
public `Cli` search/prune update regressions (including retained state, explicit
default output, empty updates and invalid-input validation), short/long help
snapshots, default and MCP test suites, Clippy for both feature sets, and the
isolated CLI stack guard. Remove the flattened adapter and its field-order
coupling only when both responsibilities are covered. An upstream compatibility
classification alone does not prove gwi's partial-update semantics are preserved.
