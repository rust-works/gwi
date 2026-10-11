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
