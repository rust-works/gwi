#!/usr/bin/env python3
"""Regression tests for MSVC frame accounting used in CLI stack reports."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

from measure_cli_stack import builder_paths, clean_root, library_assemblies, windows_frames


class FrameTests(unittest.TestCase):
    def test_allocations_saved_registers_and_direct_builder_calls(self):
        frames, calls = windows_frames('''
.seh_proc root_augment_args
 pushq %rbp
 .seh_pushreg %rbp
 subq $4096, %rsp
 .seh_stackalloc 4096
 callq child_augment_subcommands
 callq dependency
 .seh_endproc
.seh_proc child_augment_subcommands
 .seh_stackalloc 512
 callq root_augment_args
 .seh_endproc
.seh_proc dependency
 .seh_stackalloc 9999
 .seh_endproc
''')
        self.assertEqual(frames, {
            "root_augment_args": 4104, "child_augment_subcommands": 512,
            "dependency": 9999,
        })
        self.assertEqual(calls["root_augment_args"], {
            "child_augment_subcommands", "dependency",
        })
        paths = builder_paths(frames, calls)
        self.assertEqual(paths[0]["bytes"], 4616)
        self.assertEqual(len(paths[0]["frames"]), 2)

    def test_library_assembly_uses_hashed_metadata_artifact(self):
        output = json.dumps({
            "reason": "compiler-artifact",
            "target": {"name": "gwi", "kind": ["lib"]},
            "filenames": ["target/debug/libgwi.rlib",
                          "target/debug/deps/libgwi-8f7effb47400b339.rmeta"],
        })
        self.assertEqual(library_assemblies(output), [
            Path("target/debug/deps/gwi-8f7effb47400b339.s"),
        ])
        # Windows artifact names can omit the Unix library prefix.
        self.assertEqual(library_assemblies(output.replace("libgwi", "gwi")), [
            Path("target/debug/deps/gwi-8f7effb47400b339.s"),
        ])

    def test_older_checkout_is_rebuilt_in_a_shared_target(self):
        # Cargo can consider an older checkout fresh after compiling another
        # worktree into the same target. Assert the cleanup compiles its contents.
        with tempfile.TemporaryDirectory(prefix="gwi-stack-cache-") as temporary:
            root = Path(temporary)
            target = root / "target"
            for name, marker in [("newer", "BASELINE_SENTINEL_228"),
                                 ("older", "CANDIDATE_SENTINEL_228")]:
                checkout = root / name
                (checkout / "src").mkdir(parents=True)
                (checkout / "Cargo.toml").write_text(
                    '[package]\nname = "gwi"\nversion = "0.0.1"\nedition = "2021"\n',
                    encoding="utf-8",
                )
                source = checkout / "src/lib.rs"
                source.write_text(
                    f'pub fn marker() -> &\'static str {{ "{marker}" }}\n',
                    encoding="utf-8",
                )
                if name == "older":
                    os.utime(source, (1_000_000_000, 1_000_000_000))

            def compile_library(name):
                result = subprocess.run([
                    "cargo", "rustc", "--offline", "--manifest-path",
                    str(root / name / "Cargo.toml"), "--target-dir", str(target),
                    "--lib", "--message-format=json", "--", "--emit=asm",
                ], check=True, capture_output=True, text=True, encoding="utf-8")
                return result.stdout

            compile_library("newer")
            compile_library("older")
            clean_root(root / "older", target)
            output = compile_library("older")
            artifacts = [json.loads(line) for line in output.splitlines()
                         if json.loads(line).get("reason") == "compiler-artifact"]
            self.assertFalse(artifacts[-1]["fresh"])
            assembly = library_assemblies(output)[0].read_text(encoding="utf-8")
            self.assertIn("CANDIDATE_SENTINEL_228", assembly)
            self.assertNotIn("BASELINE_SENTINEL_228", assembly)

    def test_no_windows_unwind_frames(self):
        self.assertEqual(windows_frames("sub sp, sp, #4096"), ({}, {}))


if __name__ == "__main__":
    unittest.main()
