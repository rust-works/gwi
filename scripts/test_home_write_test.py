#!/usr/bin/env python3
"""Regression coverage for the empty-HOME runner, without compiling Rust."""

import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "home_write_test", Path(__file__).with_name("home-write-test.py")
)
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class HomeWriteTest(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.root_patch = patch.object(runner, "ROOT", self.root)
        self.root_patch.start()
        self.addCleanup(self.root_patch.stop)
        self.output = io.StringIO()
        self.stderr = contextlib.redirect_stderr(self.output)
        self.stderr.__enter__()
        self.addCleanup(self.stderr.__exit__, None, None, None)
        self.stdout = contextlib.redirect_stdout(io.StringIO())
        self.stdout.__enter__()
        self.addCleanup(self.stdout.__exit__, None, None, None)

    def binary(self, code):
        binary = self.root / "test-binary"
        binary.write_text(
            f"#!{sys.executable}\nimport os, pathlib, sys\n"
            "home = pathlib.Path(os.environ['HOME'])\n"
            f"pathlib.Path({str(self.root / 'observed-home')!r}).write_text(str(home))\n"
            + code + "\n"
        )
        binary.chmod(0o755)
        return binary

    def assert_home_cleaned_up(self):
        home = Path((self.root / "observed-home").read_text())
        self.assertFalse(home.exists())

    def test_clean_binary_and_environment(self):
        binary = self.binary(
            "assert home.is_dir() and not list(home.iterdir())\n"
            f"assert os.environ['INSTA_WORKSPACE_ROOT'] == {str(self.root)!r}\n"
            "assert not any(key in os.environ for key in "
            f"{runner.XDG_HOME_KEYS!r})"
        )
        with patch.dict(os.environ, {key: "/ambient" for key in runner.XDG_HOME_KEYS}):
            self.assertTrue(runner.run_binary(binary))
            self.assertEqual(os.environ["XDG_STATE_HOME"], "/ambient")
        self.assert_home_cleaned_up()

    def test_writes_fail_the_script(self):
        for code, entry in (
            ("(home / '.hidden-log').write_text('leak')", ".hidden-log"),
            ("(home / 'empty-directory').mkdir()", "empty-directory"),
            ("(home / 'dangling').symlink_to('missing')", "dangling"),
        ):
            with self.subTest(entry=entry):
                binary = self.binary(code)
                with patch.object(runner, "build_binaries", return_value=[binary]):
                    self.assertEqual(runner.main(), 1)
                self.assertIn(entry, self.output.getvalue())
                self.assert_home_cleaned_up()

    def test_failed_binary_is_checked_and_cleaned(self):
        binary = self.binary("(home / 'log').touch()\nsys.exit(7)")
        self.assertFalse(runner.run_binary(binary))
        self.assertIn("log", self.output.getvalue())
        self.assertIn("status 7", self.output.getvalue())
        self.assert_home_cleaned_up()

    def test_failed_binary_without_writes_fails(self):
        self.assertFalse(runner.run_binary(self.binary("sys.exit(9)")))
        self.assert_home_cleaned_up()

    def test_main_runs_all_binaries_and_both_features_after_failure(self):
        with patch.object(runner, "build_binaries", return_value=[Path("a"), Path("b")]) as build:
            with patch.object(runner, "run_binary", side_effect=[False, True, True, True]) as run:
                self.assertEqual(runner.main(), 1)
        self.assertEqual([call.args for call in build.call_args_list], [(None,), ("mcp",)])
        self.assertEqual(run.call_count, 4)

    def test_build_artifacts_and_real_environment(self):
        artifacts = [
            {"reason": "compiler-message"},
            {"reason": "compiler-artifact", "profile": {"test": False}, "executable": "/bin/gwi"},
            {"reason": "compiler-artifact", "profile": {"test": True}, "executable": None},
        ] + [
            {"reason": "compiler-artifact", "profile": {"test": True}, "executable": name}
            for name in ("/custom/lib", "/custom/integration", "/custom/lib")
        ]
        result = subprocess.CompletedProcess([], 0, "\n".join(map(json.dumps, artifacts)))
        with patch.object(runner.subprocess, "run", return_value=result) as run:
            self.assertEqual(runner.build_binaries("mcp"), [Path("/custom/lib"), Path("/custom/integration")])
        command = run.call_args.args[0]
        for arg in ("--lib", "--bins", "--tests", "--no-run", "--message-format=json"):
            self.assertIn(arg, command)
        self.assertEqual(command[-2:], ["--features", "mcp"])
        self.assertNotIn("env", run.call_args.kwargs)  # Cargo inherits the real HOME.
        self.assertEqual(run.call_args.kwargs["cwd"], self.root)

    def test_build_failure_and_empty_discovery_fail_closed(self):
        for status in (0, 1):
            with self.subTest(status=status):
                with patch.object(runner.subprocess, "run", return_value=subprocess.CompletedProcess([], status, "")):
                    self.assertEqual(runner.main(), 1)
        self.assertIn("no test executables", self.output.getvalue())
        self.assertIn("Cargo build failed", self.output.getvalue())


if __name__ == "__main__":
    unittest.main()
