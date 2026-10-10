#!/usr/bin/env python3
"""Exercise the hostile-environment runner with fake Cargo/test executables."""

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
    "hostile_env_test", Path(__file__).with_name("hostile-env-test.py")
)
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class HostileEnvTest(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        root_patch = patch.object(runner, "ROOT", self.root)
        root_patch.start()
        self.addCleanup(root_patch.stop)
        self.output = io.StringIO()
        for stream in (contextlib.redirect_stdout, contextlib.redirect_stderr):
            redirect = stream(self.output)
            redirect.__enter__()
            self.addCleanup(redirect.__exit__, None, None, None)

    def binary(self, code="", listing="example: test", summary="1 passed", status=0):
        binary = self.root / "fake-test"
        binary.write_text(
            f"#!{sys.executable}\nimport json, os, pathlib, sys\n"
            f"paths = {{key: os.environ[key] for key in {list(runner.PATHS)!r}}}\n"
            f"pathlib.Path({str(self.root / 'observed')!r}).write_text(json.dumps(paths))\n"
            f"if '--list' in sys.argv:\n    print({listing!r})\n    sys.exit(0)\n"
            + code + "\n"
            f"print('test result: ok. {summary}; 0 failed; 0 ignored; 0 measured; 0 filtered out')\n"
            f"sys.exit({status})\n"
        )
        binary.chmod(0o755)
        return binary

    def assert_cleaned(self):
        paths = json.loads((self.root / "observed").read_text())
        for path in paths.values():
            self.assertFalse(Path(path).parent.exists())

    def test_both_environments_override_and_clean_all_paths(self):
        developer = self.root / "developer"
        developer.mkdir()
        for case, values in runner.CASES.items():
            with self.subTest(case=case):
                code = (
                    f"assert all(os.environ[k] == v for k, v in {values!r}.items())\n"
                    f"assert os.environ['INSTA_WORKSPACE_ROOT'] == {str(self.root)!r}\n"
                    "for key, value in paths.items():\n"
                    "    path = pathlib.Path(value)\n"
                    "    assert path.parent.is_dir()\n"
                    "    if key.endswith('DIR'): path.mkdir()\n"
                    "    else: path.write_text('fixture')\n"
                )
                with patch.dict(os.environ, dict.fromkeys(runner.PATHS, str(developer / 'real'))):
                    self.assertTrue(runner.run_binary(self.binary(code), case))
                    self.assertEqual(os.environ['GWI_LOG_FILE'], str(developer / 'real'))
                self.assert_cleaned()
        self.assertEqual(list(developer.iterdir()), [])

    def test_ambient_dependent_regression_fails_main_and_runs_every_case_binary(self):
        binary = self.binary("assert 'GWI_HTTP_READ_TIMEOUT_SECS' not in os.environ")
        with patch.object(runner, "build_binaries", return_value=[binary, binary]):
            with patch.object(runner, "run_binary", wraps=runner.run_binary) as run:
                self.assertEqual(runner.main(), 1)
        self.assertEqual([call.args[1] for call in run.call_args_list],
                         ['valid', 'valid', 'malformed', 'malformed'])
        for case in runner.CASES:
            self.assertIn(f"{case}: {binary}:", self.output.getvalue())
        self.assert_cleaned()

    def test_nonzero_exit_empty_listing_and_empty_execution_fail(self):
        for options in ({'status': 7}, {'listing': '0 tests, 0 benchmarks'}, {'summary': '0 passed'}):
            with self.subTest(options=options):
                self.assertFalse(runner.run_binary(self.binary(**options), 'valid'))
                self.assert_cleaned()
        self.assertIn('status 7', self.output.getvalue())

    def test_child_summary_cannot_hide_empty_parent_selection(self):
        binary = self.binary("print('test result: ok. 1 passed; 0 failed;')", summary='0 passed')
        self.assertFalse(runner.run_binary(binary, 'valid'))
        self.assert_cleaned()

    def test_missing_binary_and_timeout_fail_with_case_diagnostics(self):
        self.assertFalse(runner.run_binary(self.root / 'missing', 'malformed'))
        def timeout(*args, **kwargs):
            (self.root / 'observed').write_text(json.dumps({key: kwargs['env'][key] for key in runner.PATHS}))
            raise subprocess.TimeoutExpired('fake', 30)
        with patch.object(runner.subprocess, 'run', side_effect=timeout):
            self.assertFalse(runner.run_binary(self.binary(), 'valid'))
        self.assertIn('malformed:', self.output.getvalue())
        self.assertIn('valid:', self.output.getvalue())
        self.assert_cleaned()

    def artifacts(self, missing=False):
        binary = self.binary()
        targets = [{'name': 'gwi', 'kind': ['lib'], 'test': True},
                   {'name': 'integration', 'kind': ['test'], 'test': True}]
        metadata = {'packages': [{'id': 'local', 'manifest_path': str(self.root / 'Cargo.toml'),
                                  'targets': targets}]}
        artifacts = [{'reason': 'compiler-message'}] + [
            {'reason': 'compiler-artifact', 'package_id': 'local', 'target': target,
             'profile': {'test': True}, 'executable': str(binary)}
            for target in (targets[:1] if missing else targets)
        ]
        artifacts += [{'reason': 'compiler-artifact', 'package_id': 'dependency',
                       'target': targets[0], 'profile': {'test': True}, 'executable': '/wrong'}]
        return json.dumps(metadata), '\n'.join(map(json.dumps, artifacts))

    def test_discovers_all_targets_and_builds_with_normal_environment(self):
        outputs = self.artifacts()
        with patch.object(runner.subprocess, 'run', side_effect=[
            subprocess.CompletedProcess([], 0, output) for output in outputs
        ]) as run:
            self.assertEqual(runner.build_binaries(), [self.root / 'fake-test'] * 2)
        for call in run.call_args_list:
            self.assertNotIn('env', call.kwargs)
            self.assertEqual(call.kwargs['cwd'], self.root)
            self.assertIn('--features', call.args[0])
        self.assertIn('--no-run', run.call_args.args[0])
        self.assertIn('--tests', run.call_args.args[0])
        self.assertIn('--lib', run.call_args.args[0])

    def test_missing_artifact_or_file_fails_discovery(self):
        for missing in (True, False):
            with self.subTest(missing=missing):
                outputs = self.artifacts(missing)
                if not missing:
                    (self.root / 'fake-test').unlink()
                with patch.object(runner, 'cargo_json', side_effect=outputs):
                    with self.assertRaisesRegex(RuntimeError, 'Missing'):
                        runner.build_binaries()

    def test_build_failure_empty_discovery_and_empty_target_set_fail_main(self):
        with patch.object(runner.subprocess, 'run', return_value=subprocess.CompletedProcess([], 3, '')):
            self.assertEqual(runner.main(), 1)
        with patch.object(runner, 'build_binaries', return_value=[]):
            self.assertEqual(runner.main(), 1)
        metadata = {'packages': [{'id': 'local', 'manifest_path': str(self.root / 'Cargo.toml'), 'targets': []}]}
        with patch.object(runner, 'cargo_json', return_value=json.dumps(metadata)):
            self.assertEqual(runner.main(), 1)


if __name__ == '__main__':
    unittest.main()
