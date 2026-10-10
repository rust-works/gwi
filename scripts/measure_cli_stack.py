#!/usr/bin/env python3
"""Record Windows MSVC debug frames and isolated CLI thread-stack outcomes.

Run from any directory; pass an explicit checkout and output directory. This
builds only the selected checkout and never changes its sources. A failed stack
budget is expected data; build errors and failure at 8 MiB are fatal.
"""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys


def windows_frames(assembly):
    """Read compiler unwind directives, without guessing from stack probes."""
    frames = {}
    calls = {}
    current = None
    for line in assembly.splitlines():
        start = re.match(r"\s*\.seh_proc\s+(\S+)", line)
        if start:
            current = start[1]
            frames[current] = 0
            calls[current] = set()
        elif current:
            allocation = re.match(r"\s*\.seh_stackalloc\s+(\d+)", line)
            if allocation:
                frames[current] += int(allocation[1])
            elif re.match(r"\s*\.seh_pushreg\s+", line):
                frames[current] += 8
            call = re.match(r"\s*(?:callq?|jmp)\s+(\S+)", line)
            if call:
                calls[current].add(call[1])
            if re.match(r"\s*\.seh_endproc", line):
                current = None
    return frames, calls


def builder_paths(frames, calls):
    """Sum direct builder-to-builder frames; this excludes dependency frames."""
    builders = {
        name for name in frames
        if any(method in name for method in (
            "augment_args", "augment_subcommands", "CommandFactory",
            "command_for_update", "from_arg_matches", "update_from_arg_matches",
        ))
    }

    def longest(name, visiting):
        if name in visiting:
            return 0, []
        children = [longest(child, visiting | {name})
                    for child in sorted(calls[name] & builders)]
        cost, path = max(children, default=(0, []), key=lambda row: row[0])
        return frames[name] + cost, [name, *path]

    return sorted(
        ({"bytes": cost, "frames": [
            {"symbol": name, "bytes": frames[name]} for name in path
        ]} for cost, path in (longest(name, set()) for name in sorted(builders))),
        key=lambda row: row["bytes"], reverse=True,
    )[:10]


def measure(checkout, output, mcp):
    output.mkdir(parents=True, exist_ok=True)
    target = output.parent / "target"
    common = ["--locked", "--manifest-path", str(checkout / "Cargo.toml"),
              "--target-dir", str(target)]
    if mcp:
        common += ["--features", "mcp"]
    # The script deliberately runs natively: Windows C dependencies and the
    # executable probe need a Windows toolchain, not merely Rust's target std.
    compilation = subprocess.run(
        ["cargo", "rustc", *common, "--lib", "--message-format=json", "--", "--emit=asm"],
        check=True, capture_output=True, text=True, encoding="utf-8", errors="replace",
    )
    libraries = [Path(filename).with_name(Path(filename).name.removeprefix("lib")).with_suffix(".s")
                 for line in compilation.stdout.splitlines()
                 if (row := json.loads(line)).get("reason") == "compiler-artifact"
                 and row.get("target", {}).get("name") == "gwi"
                 and "lib" in row["target"]["kind"]
                 for filename in row.get("filenames", []) if filename.endswith(".rlib")]
    if len(libraries) != 1 or not libraries[0].is_file():
        raise RuntimeError(f"expected one library assembly, got {libraries}")
    frames, calls = windows_frames(libraries[0].read_text(encoding="utf-8"))
    if not frames:
        raise RuntimeError("no MSVC .seh_proc frames: run this script on x86_64 Windows")
    build = subprocess.run(
        ["cargo", "test", *common, "--test", "cli_stack_test", "--no-run",
         "--message-format=json"], check=True, capture_output=True, text=True, encoding="utf-8", errors="replace",
    )
    executables = [row["executable"] for line in build.stdout.splitlines()
                   if (row := json.loads(line)).get("reason") == "compiler-artifact"
                   and row.get("target", {}).get("name") == "cli_stack_test"
                   and row.get("executable")]
    if len(executables) != 1:
        raise RuntimeError(f"expected one stack probe, got {executables}")
    outcomes = []
    # Each budget starts a fresh process, because Rust stack overflow aborts.
    for kib in (512, 768, 1024, 1280, 1536, 1792, 2048, 3072, 4096, 8192):
        child = subprocess.run(
            [executables[0], "--ignored", "--exact", "stack_probe", "--nocapture"],
            env={**os.environ, "GWI_TEST_STACK_KIB": str(kib)},
            capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=60,
        )
        (output / f"probe-{kib}.log").write_text(child.stdout + child.stderr, encoding="utf-8")
        if child.returncode == 0 and "1 passed" not in child.stdout:
            raise RuntimeError("stack probe did not execute exactly one test")
        outcomes.append({"kib": kib, "exit_code": child.returncode})
        print(f"{output.name}: {kib} KiB: exit {child.returncode}", flush=True)
    report = {
        "compiler": subprocess.check_output(["rustc", "-Vv"], text=True, encoding="utf-8"),
        "commit": subprocess.check_output(
            ["git", "-C", str(checkout), "rev-parse", "HEAD"], text=True, encoding="utf-8").strip(),
        "mcp": mcp,
        "profile": "dev (opt-level=0)",
        "largest_frames": sorted(
            ({"symbol": name, "bytes": size} for name, size in frames.items()),
            key=lambda row: row["bytes"], reverse=True)[:30],
        "largest_builder_frames": sorted(
            ({"symbol": name, "bytes": size} for name, size in frames.items()
             if "augment_args" in name or "augment_subcommands" in name),
            key=lambda row: row["bytes"], reverse=True)[:30],
        "direct_builder_paths": builder_paths(frames, calls),
        "stack_budgets": outcomes,
        "smallest_passing_tested_kib": next(
            (row["kib"] for row in outcomes if row["exit_code"] == 0), None),
    }
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2), flush=True)
    if outcomes[-1]["exit_code"] != 0:
        raise RuntimeError("probe failed at the retained 8 MiB budget")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--checkout", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--mcp", action="store_true")
    args = parser.parse_args()
    try:
        measure(args.checkout.resolve(), args.output.resolve(), args.mcp)
    except subprocess.CalledProcessError as error:
        # Cargo JSON is captured for artifact discovery; keep its diagnostics
        # visible when compilation fails rather than reporting only an exit code.
        print(error.stdout or "", file=sys.stderr)
        print(error.stderr or "", file=sys.stderr)
        raise


if __name__ == "__main__":
    main()
