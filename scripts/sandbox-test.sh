#!/usr/bin/env bash
# Run the test suite with the external network and the browser opener unavailable
# (rust-works/gwi#22), so a test that needs either fails here, as it would on a
# locked-down machine.
#
#   scripts/sandbox-test.sh                    cargo test, then cargo test --features mcp
#   scripts/sandbox-test.sh <cargo test args>  one `cargo test <args>` run
#
# The test binaries are built first, outside the sandbox (fetching crates needs the
# network); only the run is sandboxed, with `--offline`. Loopback stays reachable
# because the wiremock and MCP stdio tests bind 127.0.0.1. Inside the sandbox
# GWI_SANDBOXED=1 enables tests/sandbox_test.rs, which checks the isolation itself.
#
#   macOS  sandbox-exec with scripts/sandbox.sb, which makes /usr/bin/open unexecutable
#   Linux  a new user and network namespace (unshare -rn) with only loopback up. It needs
#          neither sudo nor root, and works as root too. The opener commands (xdg-open,
#          open, gio, ...) resolve to stubs on PATH that record the call, and the run
#          fails if any was made: the application ignores the opener's exit status, so a
#          failing stub alone would not fail a test.
#
# On Linux the run is uid 0 inside the namespace (-r maps the caller to root there), so the
# tests that need file-permission enforcement skip themselves (skip_as_root! in
# src/test_support.rs). The ordinary `cargo test` run covers them. Distributions that
# restrict unprivileged user namespaces (Ubuntu 23.10+) need
# `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`, or run this as root, which
# needs only a network namespace (unshare -n) and so no user namespace.
# Inside a container, `unshare` is blocked by the default seccomp profile: use `--privileged`.
#
# The sandbox is the backstop, not the guarantee. Every browser launch in the library goes
# through src/utils/browser_launch.rs, which panics in a unit test that has not installed a
# recorder (rust-works/gwi#34), on every platform. What only the sandbox covers are the
# integration tests in tests/, which run the real binary. There only a PATH lookup of the
# opener is intercepted on Linux, and only /usr/bin/open is denied on macOS; an opener
# invoked another way is not caught.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

# Exit status of the opener stubs and of an exec the sandbox refuses, so the probe
# below can tell "blocked" from "ran and printed usage".
blocked=126

case "$(uname -s)" in
  Darwin)
    command -v sandbox-exec >/dev/null || { echo "sandbox-exec not found" >&2; exit 2; }
    sandboxed() { sandbox-exec -f "$root/scripts/sandbox.sb" env GWI_SANDBOXED=1 "$@"; }
    opener=/usr/bin/open
    ;;
  Linux)
    for tool in unshare ip; do
      command -v "$tool" >/dev/null || { echo "$tool not found" >&2; exit 2; }
    done
    shim=$(mktemp -d)
    trap 'rm -rf "$shim"' EXIT
    calls="$shim/opener-calls"
    for name in xdg-open open gio sensible-browser x-www-browser gnome-open kde-open; do
      printf '#!/bin/sh\necho "%s $*" >>"%s"\nexit %s\n' "$name" "$calls" "$blocked" >"$shim/$name"
      chmod +x "$shim/$name"
    done
    # A new network namespace is empty, loopback included. -r gives an unprivileged caller
    # root inside a fresh user namespace, which is what lets it create the network one and
    # bring lo up. Root has that already, and skipping the user namespace sidesteps the
    # AppArmor restriction on creating one.
    unshare_flags=-rn
    [ "$(id -u)" -ne 0 ] || unshare_flags=-n
    sandboxed() {
      unshare "$unshare_flags" env "PATH=$shim:$PATH" GWI_SANDBOXED=1 sh -c \
        'ip link set lo up && exec "$@"' sh "$@"
    }
    opener=xdg-open
    ;;
  *)
    echo "no sandbox mechanism for $(uname -s)" >&2
    exit 2
    ;;
esac

# The sandbox itself must work, or the opener probe below would misreport it.
sandboxed true || {
  echo "could not start the sandbox" >&2
  [ "$(uname -s)" != Linux ] || echo "unprivileged user namespaces may be restricted: see the header of $0" >&2
  exit 2
}

# Refuse to report a pass from a sandbox that is not blocking the opener.
status=0
sandboxed sh -c "$opener" >/dev/null 2>&1 || status=$?
if [ "$status" -ne "$blocked" ]; then
  echo "sandbox check failed: $opener was not blocked (status $status)" >&2
  exit 2
fi
[ -z "${calls:-}" ] || : >"$calls"  # the probe above is not a test's call

# One `cargo test` run: build outside the sandbox, run inside it.
run_suite() {
  cargo test --no-run "$@" || return
  echo "==> sandboxed: cargo test --offline $*"
  sandboxed cargo test --offline "$@" || return
  if [ -n "${calls:-}" ] && [ -s "$calls" ]; then
    echo "the tests tried to launch a browser opener:" >&2
    cat "$calls" >&2
    return 1
  fi
}

failed=0
if [ "$#" -gt 0 ]; then
  run_suite "$@" || failed=1
else
  run_suite || failed=1
  run_suite --features mcp || failed=1
fi
exit "$failed"
