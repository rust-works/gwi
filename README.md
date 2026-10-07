# gwi

**G**oogle **W**orkspace **I**nterface: Gmail, Drive, Docs, Sheets and Slides from the
command line and as MCP tools.

> **Status: early development, not released.** gwi is being assembled from the Google
> Workspace functionality of [omni-dev](https://github.com/rust-works/omni-dev). `gwi gmail`
> works from source; the Drive commands and the MCP server are not available yet, so keep
> using `omni-dev drive` for Drive. Progress is tracked in
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

## Planned scope

- `gwi gmail`: search, read, threads, drafts, labels, attachments, sync and insert *(available from source)*
- `gwi drive`: files, Docs, Sheets, Slides, permissions and write leases
- `gwi mcp`: the Gmail and Drive MCP tools that omni-dev exposes today

## License

BSD 3-Clause. See [LICENSE](LICENSE).
