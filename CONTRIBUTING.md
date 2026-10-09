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
5. If you touch documentation or a doc comment, run `python3 scripts/check_doc_links.py`. It
   fails on a dead relative link, a dead `#anchor` and a dead `adr-NNNN*.md` name, as CI's
   `Doc links` job does. `scripts/doc-links-allowlist.txt` lists the known-dead links; do not
   add to it.
6. Open a pull request. It merges with a merge commit through a merge queue.

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
- **Linux:** `unshare -rn`, a new user and network namespace with only loopback up. `xdg-open`,
  `open`, `gio` and similar resolve to stubs that record the call; the run fails if any was
  made. Needs `unshare` and `ip` (iproute2) but not `sudo`, and runs as root or as an ordinary
  user. Inside the namespace the run is uid 0, so the tests that need file-permission
  enforcement skip themselves there (`skip_as_root!` in
  [src/test_support.rs](src/test_support.rs)); the ordinary `cargo test` run covers them.
  Distributions that restrict unprivileged user namespaces (Ubuntu 23.10+) need
  `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` first. In a container,
  Docker's default seccomp profile blocks `unshare`, so start it with `--privileged`.

The sandbox is the backstop for the browser, not the guarantee. Every browser launch goes
through `launch_detached` in [src/utils/browser_launch.rs](src/utils/browser_launch.rs), which
in a unit test panics unless the test installed a recorder with `LaunchGuard::install()`. A
test that reaches a launch it did not expect therefore fails on every platform, sandboxed or
not. Only the integration tests in `tests/`, which run the real binary, rely on the sandbox
alone.

[tests/sandbox_test.rs](tests/sandbox_test.rs) runs inside the sandbox and checks that an
external connect is refused and loopback works.

CI runs the same script in the `Sandboxed Test` job. A test that cannot run under the
sandbox must be fixed, or gated with a comment saying why.

## Permission-denial tests

Put `skip_as_root!()` first in any test that relies on file-permission denial, with a
comment explaining the failure path root would bypass. The required `Doc links` CI job
runs `python3 scripts/check_permission_tests.py`: it scans Rust files under `src/` and
`tests/` for literal three-digit octal `from_mode` calls without owner write permission
(owner digit 0, 1, 4 or 5), and requires the macro in the same function. Whitespace and
digit separators are supported; comments and strings do not count as guards. Construct
restrictive permissions in the guarded test and pass them into helpers instead of
creating restrictive modes inside helpers.

This is a textual check for rustfmt-shaped functions, not a Rust parser. Computed modes
and whether the guard executes before the permission change still need review. Run
`python3 scripts/test_check_permission_tests.py -v` to verify the scanner itself.

## Tests on Windows

The `Windows Build` job builds the release binary and compiles the lib, bin and integration
test targets with `cargo test --no-run`, with and without `--features mcp`. It does not run
them, and it does not build doctests, so a Unix-only doctest is not caught. Many tests
exercise Unix behaviour (file modes, symlinks, `flock`, `/bin/sh`) and are gated with
`#[cfg(unix)]`; but because nothing runs on Windows, an ungated test that compiles there and
would fail at run time is not caught either. What is compiled on Windows must still compile
there, so gate Unix-only imports and helpers (`std::os::unix`, a macro defined under
`cfg(unix)`) on the item that uses them, and keep the rest of a test running everywhere
where it can.
