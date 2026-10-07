# gwi

**G**oogle **W**orkspace **I**nterface: Gmail, Drive, Docs, Sheets and Slides from the
command line and as MCP tools.

> **Status: early development, not released.** gwi is being assembled from the Google
> Workspace functionality of [omni-dev](https://github.com/rust-works/omni-dev). `gwi gmail`
> works from source, and so does the Gmail MCP server; the Drive commands and tools are not
> available yet, so keep using `omni-dev drive` for Drive. Progress is tracked in
> [rust-works/omni-dev#2203](https://github.com/rust-works/omni-dev/issues/2203).

## Try it from source

```bash
cargo run -- gmail --help
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
omni-dev's directory. Drive's lease ledger is not copied yet; that arrives with the Drive
commands.

## MCP server

`gwi-mcp` serves the Gmail tools to an AI assistant over the Model Context Protocol (stdio).
It is a separate binary, built with the `mcp` feature:

```bash
cargo install --path . --features mcp   # installs `gwi` and `gwi-mcp`
```

Then register it with your MCP client, for example:

```json
{ "mcpServers": { "gwi": { "command": "gwi-mcp" } } }
```

The eight tools keep the names they had in omni-dev, so a client already configured for
`omni-dev-mcp` only needs the server command changed (and the omni-dev server's Gmail
tools removed once omni-dev drops them): `gmail_auth_status`, `gmail_account_list`,
`gmail_search`, `gmail_message_read`, `gmail_thread_read`, `gmail_label_list`,
`gmail_draft_list` and `gmail_draft_show`. They only read from Gmail: there is no tool to
send, delete or change anything in a mailbox, and none for the interactive
`gwi gmail auth login`, so sign in from a terminal first. (Some of the reading tools
accept an `output_file` to write a large result to a local file instead of returning it.)
Every tool except `gmail_account_list` takes an optional `account`.

Two optional defaults come from the `mcp` block of `~/.gwi/settings.json`: `log_level` (a
tracing directive; `RUST_LOG` wins) and `max_response_bytes` (the cap before a response is
truncated, default 100 KB, `0` for no limit). `gwi import` copies both from omni-dev.

## Planned scope

- `gwi gmail`: search, read, threads, drafts, labels, attachments, sync and insert *(available from source)*
- `gwi-mcp`: the Gmail MCP tools *(available from source)*
- `gwi drive`: files, Docs, Sheets, Slides, permissions and write leases, and their MCP tools

## License

BSD 3-Clause. See [LICENSE](LICENSE).
