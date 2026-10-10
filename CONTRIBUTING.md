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

## Checking that tests leave HOME empty

Run the same empty-HOME guard as CI's Linux `Empty HOME Test` job:

```bash
python3 scripts/home-write-test.py
```

The script supports Linux and macOS and needs Python 3 and Cargo. It builds the lib, bin
unit tests and integration tests with the normal environment, then runs the executables
reported by Cargo directly, each with a fresh temporary `HOME`. It covers both default and
`mcp` features. Cargo itself keeps the real `HOME`/`CARGO_HOME` so rustup and dependency
fetching still work. `INSTA_WORKSPACE_ROOT` points at this checkout so snapshot tests do
not invoke Cargo through rustup from the empty HOME. The test processes have inherited `XDG_CONFIG_HOME`, `XDG_DATA_HOME`,
`XDG_STATE_HOME` and `XDG_CACHE_HOME` removed so default Unix paths resolve under that HOME.

A failed test or any entry left under HOME (including hidden files, empty directories and
symlinks) fails the command; the script reports the binary and leftover top-level entries
and cleans up afterward. It does not detect transient writes that tests remove, writes to
unrelated absolute paths, or writes under a HOME that a test explicitly substitutes for its
own fixture. Doctests are excluded: Cargo's no-run JSON artifacts do not expose their
executables. Application path resolution is unchanged.

The guard's own regression tests include deliberately writing fake test binaries:

```bash
python3 scripts/test_home_write_test.py -v
```

## Testing with hostile ambient settings

Run CI's Linux `Hostile Environment Test` locally on Linux or macOS:

```bash
python3 scripts/test_hostile_env_test.py -v
python3 scripts/hostile-env-test.py
```

The runner builds the MCP-enabled library and every declared integration test target
with the normal environment, then runs their Cargo-reported executables directly.
Two sweeps apply valid inconvenient values and malformed values for request/audit
logging (including rotation, bodies and headers), HTTP timeouts, secret-command
limits and Drive lease policy. Each binary gets temporary request-log, audit-log
and backup paths, cleaned up even when tests fail; the developer's exported paths
are replaced only in the child environment. Application parsing behavior is unchanged.

Missing executables, empty test selections and failed tests fail the command with
case and binary diagnostics. Subsequent binaries and cases still run after a test
failure. Each binary has a five-minute execution limit; the CI job has a fifteen-minute
limit. Ignored tests retain their normal behavior, and doctests are excluded because
Cargo's no-run artifact stream does not expose them. This checks outcomes under
ambient settings; the separate empty-HOME guard checks leftover writes. The runner's
regressions use deliberately ambient-dependent fake executables to prove failure
propagation, environment separation and cleanup.

## Tests on Windows

The `Windows Build` job builds the release binary with `--features mcp`, then runs
`cargo test --verbose` and `cargo test --features mcp --verbose` on `windows-latest`.
Both runs execute the lib, bin and integration tests and doctests. A test that fails to
compile, link or run on Windows turns the job red.

Tests that require Unix behaviour (file modes, Unix symlinks, `flock`, `/bin/sh`, or a
particular Unix filesystem error) use `#[cfg(unix)]` with a comment explaining why.
Gate Unix-only imports and helpers on the items that use them, and keep portable
assertions running on every platform. Rust doc examples must also be portable or gate
their Unix-specific code explicitly: a normal `cargo test` run checks them too.

Windows home discovery uses the Known Folder API, which ignores `HOME`. Unit-test
settings guards explicitly route settings into scoped fixtures. Real CLI and MCP
subprocess fixtures use `GWI_HOME` and `GWI_STATE_DIR` to pin application settings
and every default state path to temporary directories on all platforms. They also
scrub credentials and ambient configuration, and pin request and audit logs. The
MCP fixture uses Cargo's absolute executable path without a Unix-specific `PATH`.
Settings/profile/import scenarios and all three MCP stdio/rejection scenarios run
on Windows; only tests of platform-specific filesystem or pipe behaviour remain
gated as described above.
