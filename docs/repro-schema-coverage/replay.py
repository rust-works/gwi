#!/usr/bin/env python3
"""Replay the description-only coverage mapping difference in isolated copies."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess

SCRIPT = r"""
set -euxo pipefail
cd "$REPRO_WT"
cargo() { command cargo "+$REPRO_TOOLCHAIN" "$@"; }
rustc "+$REPRO_TOOLCHAIN" -Vv
cargo -V
cargo llvm-cov --version
export CARGO_TARGET_DIR="$REPRO_OUT/target"
eval "$(cargo llvm-cov show-env --sh)"
cargo llvm-cov clean --workspace
cargo test --locked --all-features --workspace
cargo llvm-cov report --lcov --output-path "$REPRO_OUT/coverage.lcov"
cargo llvm-cov clean --profraw-only
cargo llvm-cov report --json --output-path "$REPRO_OUT/coverage.json"
RUSTC_BOOTSTRAP=1 cargo rustc -p span-case --lib -- \
    -Zdump-mir=InstrumentCoverage -Zdump-mir-dir="$REPRO_OUT/mir" --emit=llvm-ir
RUSTC_BOOTSTRAP=1 cargo rustc -p span-case --lib -- \
    -Zunpretty=expanded > "$REPRO_OUT/expanded.rs"
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="new directory for sources and evidence")
    parser.add_argument("--toolchain", default="1.99.0")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    source = Path(__file__).resolve().parent
    summary = {}
    for name, description in [("plain", '"x"'), ("bang", '"x!"')]:
        case_output = output / name
        workspace = case_output / "source"
        workspace.mkdir(parents=True)
        for file in ["Cargo.toml", "Cargo.lock"]:
            shutil.copy2(source / file, workspace / file)
        for directory in ["macros", "case"]:
            shutil.copytree(source / directory, workspace / directory)
        fixture = workspace / "case/src/lib.rs"
        fixture.write_text(fixture.read_text().replace('"x"', description))
        env = os.environ.copy()
        for key in ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "LLVM_PROFILE_FILE"]:
            env.pop(key, None)
        env.update(REPRO_WT=str(workspace), REPRO_OUT=str(case_output),
                   REPRO_TOOLCHAIN=args.toolchain)
        log_path = case_output / "run.log"
        with log_path.open("w") as log:
            result = subprocess.run(["bash", "-c", SCRIPT], env=env, stdout=log,
                                    stderr=subprocess.STDOUT)
        if result.returncode:
            raise SystemExit(f"{name}: replay failed ({result.returncode}); see {log_path}")
        data = json.loads((case_output / "coverage.json").read_text())
        functions = [f for unit in data["data"] for f in unit["functions"]
                     if "handler_metadata" in f["name"]]
        summary[name] = functions
        print(f"{name}: {len(functions)} mapped fallback(s)", flush=True)
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    if summary["plain"] or len(summary["bang"]) != 1:
        raise SystemExit("Historical mapping difference did not reproduce; inspect the evidence.")
    fallback = summary["bang"][0]
    if (fallback["count"] != 0 or len(fallback["regions"]) != 1
            or fallback["regions"][0][4] != 0):
        raise SystemExit("Unexpected fallback count or regions; inspect the evidence.")
    print("Reproduced: unchanged unused fallback gains a region after adding '!'.")


if __name__ == "__main__":
    main()
