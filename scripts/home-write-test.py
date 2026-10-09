#!/usr/bin/env python3
"""Build normally, then run test executables with HOME required to stay empty."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
XDG_HOME_KEYS = ("XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME")


def build_binaries(features):
    """Use Cargo's artifacts, not target-directory names or a presumed target path."""
    command = [
        "cargo", "test", "--manifest-path", str(ROOT / "Cargo.toml"),
        "--lib", "--bins", "--tests", "--no-run", "--message-format=json",
    ]
    if features:
        command += ["--features", features]
    result = subprocess.run(command, cwd=ROOT, stdout=subprocess.PIPE, text=True)
    if result.returncode:
        raise RuntimeError(f"Cargo build failed (status {result.returncode})")
    binaries = []
    for line in result.stdout.splitlines():
        artifact = json.loads(line)
        if (artifact.get("reason") == "compiler-artifact"
                and artifact.get("profile", {}).get("test")
                and artifact.get("executable")):
            binary = Path(artifact["executable"])
            if binary not in binaries:
                binaries.append(binary)
    if not binaries:
        raise RuntimeError("Cargo reported no test executables; refusing an empty check")
    return binaries


def run_binary(binary):
    """Check leftovers even if the tests fail; TemporaryDirectory owns cleanup."""
    with tempfile.TemporaryDirectory(prefix="gwi-test-home-") as scratch:
        home = Path(scratch)
        env = os.environ.copy()
        env["HOME"] = str(home)
        # insta otherwise calls `cargo metadata`; rustup would initialize .rustup
        # in the scratch HOME before falling back to the same workspace root.
        env["INSTA_WORKSPACE_ROOT"] = str(ROOT)
        for key in XDG_HOME_KEYS:
            env.pop(key, None)
        print(f"==> empty HOME: {binary}", flush=True)
        result = subprocess.run([str(binary)], cwd=ROOT, env=env)
        # iterdir includes hidden entries, empty directories and dangling symlinks.
        entries = sorted(home.iterdir())
        if entries:
            print(f"{binary} left entries under HOME:", file=sys.stderr)
            for entry in entries:
                print(f"  {entry.relative_to(home)}", file=sys.stderr)
        if result.returncode:
            print(f"{binary} failed (status {result.returncode})", file=sys.stderr)
        return not entries and result.returncode == 0


def main():
    if sys.platform not in ("linux", "darwin"):
        print("The empty-HOME check supports Linux and macOS", file=sys.stderr)
        return 2
    success = True
    try:
        for features in (None, "mcp"):
            for binary in build_binaries(features):
                # Keep checking subsequent binaries after a failure.
                if not run_binary(binary):
                    success = False
    except (OSError, RuntimeError, ValueError) as error:
        print(error, file=sys.stderr)
        return 1
    return 0 if success else 1


if __name__ == "__main__":
    sys.exit(main())
