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
#   Linux  a new network namespace (sudo unshare --net) with only loopback up. The
#          opener commands (xdg-open, open, gio, ...) resolve to stubs on PATH that
#          record the call, and the run fails if any was made: the application ignores
#          the opener's exit status, so a failing stub alone would not fail a test.
#
# Only a PATH lookup of the opener is intercepted on Linux, and only /usr/bin/open is
# denied on macOS; an opener invoked another way is not caught.
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
    [ "$(id -u)" -ne 0 ] || { echo "run as an unprivileged user: the tests assume non-root file permissions" >&2; exit 2; }
    for tool in sudo unshare setpriv ip; do
      command -v "$tool" >/dev/null || { echo "$tool not found" >&2; exit 2; }
    done
    sudo -n true 2>/dev/null || { echo "passwordless sudo is needed to create a network namespace" >&2; exit 2; }
    shim=$(mktemp -d)
    trap 'rm -rf "$shim"' EXIT
    calls="$shim/opener-calls"
    for name in xdg-open open gio sensible-browser x-www-browser gnome-open kde-open; do
      printf '#!/bin/sh\necho "%s $*" >>"%s"\nexit %s\n' "$name" "$calls" "$blocked" >"$shim/$name"
      chmod +x "$shim/$name"
    done
    # A network namespace needs root. Create it with sudo, bring loopback up, then drop
    # back to this user so file-permission tests behave as they do unsandboxed.
    sandboxed() {
      sudo -n -E unshare --net env "PATH=$shim:$PATH" "HOME=$HOME" GWI_SANDBOXED=1 sh -c \
        'uid=$1 gid=$2; shift 2; ip link set lo up && exec setpriv --reuid="$uid" --regid="$gid" --init-groups "$@"' \
        sh "$(id -u)" "$(id -g)" "$@"
    }
    opener=xdg-open
    ;;
  *)
    echo "no sandbox mechanism for $(uname -s)" >&2
    exit 2
    ;;
esac

# The sandbox itself must work, or the opener probe below would misreport it.
sandboxed true || { echo "could not start the sandbox" >&2; exit 2; }

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
