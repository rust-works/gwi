#!/usr/bin/env python3
"""Build normally, then sweep MCP lib/integration tests under hostile settings."""

import json
import os
import re
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
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


def run_binary(binary, case):
    """No filters: run every nonignored test, with scratch paths even on failure."""
    print(f"==> hostile environment ({case}): {binary}", flush=True)
    try:
        with tempfile.TemporaryDirectory(prefix="gwi-hostile-env-") as scratch:
            env = os.environ.copy()
            env.update(CASES[case])
            env.update({key: str(Path(scratch) / name) for key, name in PATHS.items()})
            env["INSTA_WORKSPACE_ROOT"] = str(ROOT)
            listed = subprocess.run(
                [str(binary), "--list"],
                cwd=ROOT, env=env, stdout=subprocess.PIPE, text=True, timeout=30,
            )
            # --list includes ignored tests, so check the actual execution summary too.
            if listed.returncode or not any(line.endswith(": test") for line in listed.stdout.splitlines()):
                raise RuntimeError("Failed or empty test listing")
            result = subprocess.run(
                [str(binary), "--color", "never"], cwd=ROOT, env=env,
                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=300,
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
