#!/usr/bin/env python3
"""Build normally, then sweep MCP lib/integration tests under hostile settings."""

import json
import os
import re
import signal
from pathlib import Path
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
APP_DIR_KEYS = ("GWI_HOME", "GWI_STATE_DIR")
LIST_TIMEOUT = 30
TEST_TIMEOUT = 300
TERMINATION_GRACE = 1
CLEANUP_TIMEOUT = 2
# Same valid inconvenient values as the #120 audit. Malformed values exercise
# existing fallback/error semantics; paths remain valid and owned by this runner.
VALID = {
    "GWI_LOG_DISABLE": "1",
    "GWI_LOG_MAX_SIZE": "1",
    "GWI_LOG_KEEP_FILES": "0",
    "GWI_LOG_BODIES": "1",
    "GWI_LOG_HEADERS": "1",
    "GWI_HTTP_CONNECT_TIMEOUT_SECS": "1",
    "GWI_HTTP_READ_TIMEOUT_SECS": "1",
    "GWI_SECRET_COMMAND_TIMEOUT_SECS": "1",
    "GWI_SECRET_COMMAND_TTL_SECS": "0",
    "GWI_DRIVE_LEASE_EXPIRY_MINUTES": "1",
    "GWI_DRIVE_LEASE_BIOMETRICS_ONLY": "1",
    "GWI_DRIVE_LEASE_ALLOW_HEADLESS": "0",
}
CASES = {"valid": VALID, "malformed": dict.fromkeys(VALID, "not-a-value")}
PATHS = {
    "GWI_LOG_FILE": "request.jsonl",
    "GWI_AUDIT_LOG_FILE": "audit.jsonl",
    "GWI_DRIVE_LEASE_BACKUP_DIR": "backups",
}


def cargo_json(arguments):
    result = subprocess.run(
        ["cargo", *arguments, "--manifest-path", str(ROOT / "Cargo.toml")],
        cwd=ROOT, stdout=subprocess.PIPE, text=True,
    )
    if result.returncode:
        raise RuntimeError(f"Cargo failed (status {result.returncode})")
    return result.stdout


def target_key(target):
    kind = target.get("kind", [])
    if "lib" in kind:
        return ("lib", target["name"])
    if "test" in kind:
        return ("test", target["name"])
    return None


def build_binaries():
    """Cross-check Cargo artifacts against declared targets, not filename globs."""
    metadata = json.loads(cargo_json([
        "metadata", "--no-deps", "--format-version=1", "--features", "mcp",
    ]))
    package = next(p for p in metadata["packages"]
                   if Path(p["manifest_path"]).resolve() == (ROOT / "Cargo.toml").resolve())
    expected = {target_key(t) for t in package["targets"] if target_key(t) and t.get("test", True)}
    if not expected or not any(kind == "lib" for kind, _ in expected):
        raise RuntimeError("No library test target; refusing an empty check")
    output = cargo_json([
        "test", "--features", "mcp", "--lib", "--tests", "--no-run", "--message-format=json",
    ])
    found = {}
    for line in output.splitlines():
        artifact = json.loads(line)
        if (artifact.get("reason") == "compiler-artifact"
                and artifact.get("package_id") == package["id"]
                and artifact.get("profile", {}).get("test")):
            key = target_key(artifact["target"])
            if key in expected and artifact.get("executable"):
                found[key] = Path(artifact["executable"])
    missing = expected - found.keys()
    if missing:
        raise RuntimeError(f"Missing test executables: {sorted(missing)}")
    for binary in found.values():
        if not binary.is_file() or not os.access(binary, os.X_OK):
            raise RuntimeError(f"Missing or non-executable test binary: {binary}")
    return [found[key] for key in sorted(expected)]


class DiscoveryTimeout(RuntimeError):
    """The current termination phase exhausted its discovery budget."""


def session_members(session, deadline):
    """Discover live members without trusting platform-specific ps SID fields."""
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise DiscoveryTimeout("Timed out discovering test session")
    listing = subprocess.run(
        ["/bin/ps", "-axo", "pid=,stat="], capture_output=True,
        text=True, timeout=remaining,
    )
    if listing.returncode:
        raise RuntimeError("Failed to inspect test session")
    members = []
    for line in listing.stdout.splitlines():
        if time.monotonic() >= deadline:
            raise DiscoveryTimeout("Timed out discovering test session")
        pid, state = line.split()
        if state.startswith("Z"):
            continue
        pid = int(pid)
        try:
            if os.getsid(pid) == session:
                members.append(pid)
        except ProcessLookupError:
            pass
    return members


def signal_session(session, members, sig, *, deadline=None):
    """Recheck ownership after discovery; group changes do not change ownership."""
    if session == os.getsid(0):
        raise RuntimeError("Refusing to signal runner session")
    for pid in members:
        if deadline is not None and time.monotonic() >= deadline:
            raise DiscoveryTimeout("Timed out signalling test session")
        try:
            if os.getsid(pid) == session:
                os.kill(pid, sig)
        except ProcessLookupError:
            pass


def terminate_session(session):
    """Rescan during grace and escalation, with bounded discovery and exit waits."""
    known = []
    for sig, duration in ((signal.SIGTERM, TERMINATION_GRACE),
                          (signal.SIGKILL, CLEANUP_TIMEOUT)):
        deadline = time.monotonic() + duration
        if sig == signal.SIGKILL and known:
            # Escalate already-discovered members before another ps invocation
            # can exhaust the cleanup budget under system load. Revalidate as usual.
            signal_session(session, known, sig, deadline=deadline)
        while True:
            try:
                members = session_members(session, deadline)
                if not members:
                    return
                known = members
                signal_session(session, members, sig, deadline=deadline)
            except (DiscoveryTimeout, subprocess.TimeoutExpired):
                # A grace-period discovery deadline must not skip SIGKILL.
                break
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            time.sleep(min(0.02, remaining))
            if time.monotonic() >= deadline:
                break
    raise RuntimeError("Test session still live after SIGKILL")


def run_isolated(arguments, *, env, timeout, stderr=None):
    """Bound execution and timeout cleanup of a dedicated POSIX session."""
    process = subprocess.Popen(
        arguments, cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=stderr,
        text=True, start_new_session=True,
    )
    try:
        stdout, _ = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        # communicate has not reaped the leader on this timeout path. Keep
        # it unreaped until discovery/escalation finishes, anchoring the SID.
        try:
            terminate_session(process.pid)
        finally:
            try:
                process.communicate(timeout=CLEANUP_TIMEOUT)
            except subprocess.TimeoutExpired:
                # A new-session descendant may still hold stdout. Do not wait for
                # EOF forever; separately kill/reap the owned executable even
                # if signalling the group failed.
                process.stdout.close()
                process.kill()
                process.wait(timeout=CLEANUP_TIMEOUT)
        raise
    return subprocess.CompletedProcess(arguments, process.returncode, stdout)


def run_binary(binary, case):
    """No filters: run every nonignored test, with scratch paths even on failure."""
    print(f"==> hostile environment ({case}): {binary}", flush=True)
    try:
        with tempfile.TemporaryDirectory(prefix="gwi-hostile-env-") as scratch:
            env = os.environ.copy()
            # Preserve platform defaults instead of inheriting an ambient installation.
            for key in APP_DIR_KEYS:
                env.pop(key, None)
            env.update(CASES[case])
            env.update({key: str(Path(scratch) / name) for key, name in PATHS.items()})
            env["INSTA_WORKSPACE_ROOT"] = str(ROOT)
            listed = run_isolated(
                [str(binary), "--list"],
                env=env, timeout=LIST_TIMEOUT,
            )
            # --list includes ignored tests, so check the actual execution summary too.
            if listed.returncode or not any(line.endswith(": test") for line in listed.stdout.splitlines()):
                raise RuntimeError("Failed or empty test listing")
            result = run_isolated(
                [str(binary), "--color", "never"], env=env,
                stderr=subprocess.STDOUT, timeout=TEST_TIMEOUT,
            )
            print(result.stdout, end="", flush=True)
            summaries = re.findall(r"^test result: ok\. (\d+) passed;", result.stdout, re.MULTILINE)
            if result.returncode or not summaries or int(summaries[-1]) == 0:
                raise RuntimeError(f"Failed tests or empty selection (status {result.returncode})")
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"{case}: {binary}: {error}", file=sys.stderr)
        return False
    return True


def main():
    if sys.platform not in ("linux", "darwin"):
        print("The hostile-environment check supports Linux and macOS", file=sys.stderr)
        return 2
    try:
        binaries = build_binaries()
        if not binaries:
            raise RuntimeError("No test executables; refusing an empty check")
    except (OSError, RuntimeError, ValueError, KeyError, StopIteration) as error:
        print(error, file=sys.stderr)
        return 1
    success = True
    for case in CASES:
        for binary in binaries:
            if not run_binary(binary, case):
                success = False
    return 0 if success else 1


if __name__ == "__main__":
    sys.exit(main())
