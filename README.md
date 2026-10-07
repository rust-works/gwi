# gwi

**G**oogle **W**orkspace **I**nterface: Gmail, Drive, Docs, Sheets and Slides from the
command line and as MCP tools.

> **Status: early development, not released.** gwi is being assembled from the Google
> Workspace functionality of [omni-dev](https://github.com/rust-works/omni-dev). `gwi gmail`
> works from source; the Drive commands, the MCP server and the command that imports your
> existing omni-dev accounts are not available yet, so keep using `omni-dev drive` and
> `omni-dev gmail` for those. Progress is tracked in
> [rust-works/omni-dev#2203](https://github.com/rust-works/omni-dev/issues/2203).

## Try it from source

```bash
cargo run -- gmail --help
```

Configuration lives in `~/.gwi/settings.json` (not omni-dev's `~/.omni-dev`), and
environment variables use the `GWI_` prefix.

## Planned scope

- `gwi gmail`: search, read, threads, drafts, labels, attachments, sync and insert *(available from source)*
- `gwi drive`: files, Docs, Sheets, Slides, permissions and write leases
- `gwi mcp`: the Gmail and Drive MCP tools that omni-dev exposes today

## License

BSD 3-Clause. See [LICENSE](LICENSE).
