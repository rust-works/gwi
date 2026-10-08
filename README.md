# gwi

**G**oogle **W**orkspace **I**nterface: Gmail, Drive, Docs, Sheets and Slides from the
command line and as MCP tools.

> **Status: early development, not released.** gwi is being assembled from the Google
> Workspace functionality of [omni-dev](https://github.com/rust-works/omni-dev). `gwi gmail`
> and `gwi drive` work from source, and so does the MCP server with its Gmail and Drive tools.
> Progress is tracked in
> [rust-works/omni-dev#2203](https://github.com/rust-works/omni-dev/issues/2203).

## Try it from source

```bash
cargo run -- gmail --help
cargo run -- drive --help
```

Configuration lives in `~/.gwi/settings.json` (not omni-dev's `~/.omni-dev`), and
environment variables use the `GWI_` prefix.

### Coming from omni-dev

If you already configured Gmail in omni-dev, copy it across instead of logging in again:

```bash
gwi import --dry-run   # show what would be imported; writes nothing
gwi import             # copy the Gmail and Drive accounts and Google variables
```

It only reads omni-dev's `~/.omni-dev/settings.json` (use `--source PATH` for another file),
never changes it, never overwrites a different value you already have in gwi (pass
`--force` to), and never prints a secret. Anything gwi already has with the same value is
reported as unchanged, so running it twice is safe. It warns about any `*_file` secret path
that points inside `~/.omni-dev/`, since that file would be lost if you later remove
omni-dev's directory.

The same command copies Drive's lease ledger (`omni-dev/lease-ledger.jsonl` in the state
directory; `--source-ledger PATH` for another file), so a lease taken with
`omni-dev drive lease acquire` works and can be restored through `gwi drive`. Every lease is
copied, expired ones too, because a restore needs them. A lease that is still live carries
over unchanged and stays valid until it expires, in both tools: the two ledgers are
copies, so releasing it in omni-dev does not release it in gwi. Audit history is not
copied. See [Coming from omni-dev](docs/drive.md#coming-from-omni-dev) for the details.

`--dry-run` lists conflicts as part of its preview and exits 0 for them, in both the settings
and the lease ledger, with a note saying so. It is a preview, not a check: the real run
still exits 1 on a conflict until you pass `--force`, so read the report before
`gwi import --dry-run && gwi import` (a dry run fails only when the import itself does).

A folder you synced with `omni-dev drive sync` needs nothing: `gwi drive sync` reads its
`.omni-dev-sync.json` and continues in `.gwi-sync.json`, leaving the old file alone.

## MCP server

`gwi-mcp` serves the Gmail and Drive tools to an AI assistant over the Model Context Protocol (stdio).
It is a separate binary, built with the `mcp` feature:

```bash
cargo install --path . --features mcp   # installs `gwi` and `gwi-mcp`
```

Then register it with your MCP client, for example:

```json
{ "mcpServers": { "gwi": { "command": "gwi-mcp" } } }
```

The 23 tools keep the names they had in omni-dev, so a client already configured for
`omni-dev-mcp` only needs the server command changed (and the omni-dev server's Gmail and
Drive tools removed once omni-dev drops them). The eight Gmail tools are `gmail_auth_status`, `gmail_account_list`,
`gmail_search`, `gmail_message_read`, `gmail_thread_read`, `gmail_label_list`,
`gmail_draft_list` and `gmail_draft_show`. They only read from Gmail: there is no tool to
send, delete or change anything in a mailbox, and none for the interactive
`gwi gmail auth login`, so sign in from a terminal first. (Some of the reading tools
accept an `output_file` to write a large result to a local file instead of returning it.)

The 15 Drive tools are:

- Read-only: `drive_auth_status`, `drive_account_list`, `drive_search`, `drive_dedupe`,
  `drive_file_read`, `drive_docs_info`, `drive_docs_read`, `drive_sheets_info` and
  `drive_sheets_read`.
- Writing: `drive_docs_replace`, `drive_docs_append`, `drive_sheets_write`,
  `drive_sheets_append`, `drive_sheets_clear` and `drive_lease_acquire`. Each is behind the
  write gate: the folder permission rules in `settings.json` must allow the target, and a
  write lease (from `drive_lease_acquire` or `gwi drive lease acquire`) is required unless
  the call is a dry run or the rule says `require_lease: false`. Acquiring a lease prompts
  for device-owner authentication (Touch ID or the account password), which exists on macOS
  only: elsewhere `drive_lease_acquire` fails unless the operator has set `allow_headless`
  in `settings.json`. The consent policy
  (`allow_headless`, `biometrics_only`), the ledger path and the permission rules cannot be
  set from a tool call. No MCP tool can create, move, rename, trash or delete a file, and
  the interactive `gwi drive auth login` is CLI-only too.

Every tool except `gmail_account_list` and `drive_account_list` takes an optional `account`.

Two optional defaults come from the `mcp` block of `~/.gwi/settings.json`: `log_level` (a
tracing directive; `RUST_LOG` wins) and `max_response_bytes` (the cap before a response is
truncated, default 100 KB, `0` for no limit). `gwi import` copies both from omni-dev.

## Planned scope

- `gwi gmail`: search, read, threads, drafts, labels, attachments, sync and insert *(available from source)*
- `gwi-mcp`: the Gmail MCP tools *(available from source)*
- `gwi drive`: files, Docs, Sheets, Slides, permissions and write leases *(available from source)*
- `gwi-mcp`: the Drive MCP tools *(available from source)*

## License

BSD 3-Clause. See [LICENSE](LICENSE).
