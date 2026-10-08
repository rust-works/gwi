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
# because the wiremock and MCP stdio tests bind 127.0.0.1.
#
#   macOS  sandbox-exec with scripts/sandbox.sb
#   Linux  a new network namespace (sudo unshare --net) with only loopback up, and a
#          PATH whose opener commands (xdg-open, open, gio, sensible-browser) fail
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

# Exit status of the opener stubs and of an exec the sandbox refuses, so the probe
# below can tell "blocked" from "ran and printed usage".
blocked=126

case "$(uname -s)" in
  Darwin)
    command -v sandbox-exec >/dev/null || { echo "sandbox-exec not found" >&2; exit 2; }
    sandboxed() { sandbox-exec -f "$root/scripts/sandbox.sb" "$@"; }
    opener=/usr/bin/open
    ;;
  Linux)
    [ "$(id -u)" -ne 0 ] || { echo "run as an unprivileged user: the tests assume non-root file permissions" >&2; exit 2; }
    for tool in sudo unshare setpriv ip; do
      command -v "$tool" >/dev/null || { echo "$tool not found" >&2; exit 2; }
    done
    shim=$(mktemp -d)
    trap 'rm -rf "$shim"' EXIT
    for name in xdg-open open gio sensible-browser; do
      printf '#!/bin/sh\necho "%s: blocked by scripts/sandbox-test.sh" >&2\nexit %s\n' \
        "$name" "$blocked" >"$shim/$name"
      chmod +x "$shim/$name"
    done
    # A network namespace needs root. Create it with sudo, bring loopback up, then drop
    # back to this user so file-permission tests behave as they do unsandboxed.
    sandboxed() {
      sudo -E unshare --net env "PATH=$shim:$PATH" sh -c \
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

# Refuse to report a pass from a sandbox that is not blocking the opener.
status=0
sandboxed sh -c "$opener" >/dev/null 2>&1 || status=$?
if [ "$status" -ne "$blocked" ]; then
  echo "sandbox check failed: $opener was not blocked (status $status)" >&2
  exit 2
fi

if [ "$#" -gt 0 ]; then
  runs=("$*")
else
  runs=("" "--features mcp")
fi

for run in "${runs[@]}"; do
  # shellcheck disable=SC2086  # $run is a space-separated argument list
  cargo test --no-run $run
  echo "==> sandboxed: cargo test --offline $run"
  # shellcheck disable=SC2086
  sandboxed cargo test --offline $run
done
