# Contributing to gwi

Thanks for your interest. gwi is in its early extraction phase, so the best way to help right
now is to comment on the design in
[rust-works/omni-dev#2203](https://github.com/rust-works/omni-dev/issues/2203).

## Workflow

1. Fork and branch from `main`.
2. Keep changes focused. Run `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`
   and `cargo test`.
3. Use [conventional commits](https://www.conventionalcommits.org/); see
   [.omni-dev/commit-guidelines.md](.omni-dev/commit-guidelines.md) and
   [docs/STYLE_GUIDE.md](docs/STYLE_GUIDE.md).
4. Add a changelog fragment, `changelog.d/<issue>.<type>.md`, instead of editing
   [CHANGELOG.md](CHANGELOG.md) (see [changelog.d/README.md](changelog.d/README.md)). A change
   with no user-visible effect puts `[no changelog]` in the PR body instead.
5. Open a pull request. It merges with a merge commit through a merge queue.

## Running the tests in a sandbox

The suite must pass with no external network and no browser opener (the OAuth flow would
launch one), so a test that quietly needs either is caught here and not on someone else's
machine. One command runs `cargo test` and then `cargo test --features mcp` that way:

```bash
scripts/sandbox-test.sh
```

Pass `cargo test` arguments to run a single, narrower invocation, for example
`scripts/sandbox-test.sh --features mcp gmail::`.

The test binaries are built first, outside the sandbox, because fetching crates needs the
network; only the run is sandboxed, with `--offline`. Loopback stays reachable because the
wiremock and MCP stdio tests bind `127.0.0.1`. The script first checks that the opener is
really blocked and refuses to run otherwise.

- **macOS:** `sandbox-exec` with [scripts/sandbox.sb](scripts/sandbox.sb). The network is denied
  except loopback, and `/usr/bin/open` cannot be executed.
- **Linux:** `sudo unshare --net` with only loopback up, then back to your user. `xdg-open`,
  `open`, `gio` and similar resolve to stubs that record the call; the run fails if any was
  made. Needs passwordless `sudo`, `unshare`, `setpriv` and `ip` (iproute2), and must run as a
  non-root user.

[tests/sandbox_test.rs](tests/sandbox_test.rs) runs inside the sandbox and checks that an
external connect is refused and loopback works.

CI runs the same script in the `Sandboxed Test` job. A test that cannot run under the
sandbox must be fixed, or gated with a comment saying why.
