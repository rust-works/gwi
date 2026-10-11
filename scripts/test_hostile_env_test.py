#!/usr/bin/env python3
"""Exercise the hostile-environment runner with fake Cargo/test executables."""

import contextlib
import importlib.util
import io
import json
import os
import signal
from pathlib import Path
import subprocess
import sys
import tempfile
import time
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

    def test_application_overrides_are_scrubbed_in_listing_and_execution(self):
        with tempfile.TemporaryDirectory() as ambient:
            overrides = {key: str(Path(ambient) / key)
                         for key in ("GWI_HOME", "GWI_STATE_DIR")}
            for value in overrides.values():
                Path(value).mkdir()
            for case in runner.CASES:
                for status in (0, 7):
                    with self.subTest(case=case, status=status):
                        binary = self.binary(status=status)
                        # Insert before --list so both child invocations check isolation.
                        script = binary.read_text().replace(
                            "if '--list' in sys.argv:",
                            "for key in ('GWI_HOME', 'GWI_STATE_DIR'):\n"
                            "    if key in os.environ:\n"
                            "        (pathlib.Path(os.environ[key]) / 'leak').touch()\n"
                            "        sys.exit(9)\n"
                            "if '--list' in sys.argv:"
                        )
                        binary.write_text(script)
                        with patch.dict(os.environ, overrides):
                            self.assertEqual(runner.run_binary(binary, case), status == 0)
                            self.assertEqual({k: os.environ[k] for k in overrides}, overrides)
                        self.assert_cleaned()
                        for value in overrides.values():
                            self.assertEqual(list(Path(value).iterdir()), [])
        self.assertFalse(Path(ambient).exists())

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
        with patch.object(runner, 'run_isolated', side_effect=timeout):
            self.assertFalse(runner.run_binary(self.binary(), 'valid'))
        self.assertIn('malformed:', self.output.getvalue())
        self.assertIn('valid:', self.output.getvalue())
        self.assert_cleaned()

    def test_signal_denial_is_only_ignored_for_verified_dead_macos_group(self):
        for platform, listing, denied in (
            ('darwin', '123 Z\n999 S\n', False),
            ('darwin', '999 S\n', False),
            ('darwin', '123 S\n', True),
            ('linux', '', True),
        ):
            with self.subTest(platform=platform, listing=listing):
                with patch.object(runner.sys, 'platform', platform), \
                        patch.object(runner.os, 'killpg', side_effect=PermissionError), \
                        patch.object(runner.subprocess, 'run', return_value=
                                     subprocess.CompletedProcess([], 0, listing)):
                    if denied:
                        with self.assertRaises(PermissionError):
                            runner.signal_group(123, signal.SIGKILL)
                    else:
                        runner.signal_group(123, signal.SIGKILL)

    def assert_terminated(self, pid):
        # Orphans belong to the OS reaper. Zombies have exited and cannot retain
        # pipes, ports or scratch files, even with a slow container init process.
        deadline = time.monotonic() + 2
        while True:
            if sys.platform == 'linux':
                try:
                    state = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[0]
                except (FileNotFoundError, ProcessLookupError):
                    # Linux can report ESRCH if the process exits during the read.
                    state = ''
            else:
                state = subprocess.run(
                    ['ps', '-o', 'stat=', '-p', str(pid)], capture_output=True,
                    text=True, timeout=2,
                ).stdout.strip()
            if not state or state.startswith('Z'):
                return
            self.assertLess(time.monotonic(), deadline, (pid, state))
            time.sleep(0.01)

    def test_assert_terminated_handles_linux_process_disappearance(self):
        for error in (FileNotFoundError, ProcessLookupError):
            with self.subTest(error=error):
                with patch.object(sys, 'platform', 'linux'), \
                        patch.object(Path, 'read_text', side_effect=['123 (child) S', error()]) as read, \
                        patch.object(time, 'sleep'):
                    self.assert_terminated(123)
                self.assertEqual(read.call_count, 2)
        with patch.object(sys, 'platform', 'linux'), \
                patch.object(Path, 'read_text', side_effect=PermissionError):
            with self.assertRaises(PermissionError):
                self.assert_terminated(123)

    @unittest.skipUnless(sys.platform in ('linux', 'darwin'), 'POSIX runner')
    def test_real_timeout_terminates_group_before_continuing(self):
        for phase, resistant, leader_exits, retain_output in (
            ('list', False, False, True),
            ('run', False, False, True),
            ('run', True, False, True),
            ('list', True, True, True),
            ('run', True, False, False),
        ):
            with self.subTest(phase=phase, resistant=resistant,
                              leader_exits=leader_exits, retain_output=retain_output):
                records = self.root / 'processes.jsonl'
                records.unlink(missing_ok=True)
                child_code = (
                    "import os, pathlib, signal, time; "
                    + ("signal.signal(signal.SIGTERM, signal.SIG_IGN); " if resistant else "")
                    + f"pathlib.Path({str(self.root / 'ready')!r}).write_text('ready'); "
                    + "time.sleep(20)"
                )
                bad = self.root / 'hung-test'
                bad.write_text(
                    f"#!{sys.executable}\n"
                    "import json, os, pathlib, subprocess, sys, time\n"
                    f"if ('--list' in sys.argv) != {phase == 'list'!r}:\n"
                    "    print('example: test' if '--list' in sys.argv else 'test result: ok. 1 passed;')\n"
                    "    sys.exit(0)\n"
                    f"ready = pathlib.Path({str(self.root / 'ready')!r})\n"
                    "ready.unlink(missing_ok=True)\n"
                    f"child = subprocess.Popen([sys.executable, '-c', {child_code!r}]"
                    + (")\n" if retain_output else ", stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)\n")
                    + "while not ready.exists(): time.sleep(0.005)\n"
                    + f"with open({str(records)!r}, 'a') as record:\n"
                    + "    record.write(json.dumps({'parent': os.getpid(), 'child': child.pid, "
                    + "'group': os.getpgrp(), 'scratch': str(pathlib.Path(os.environ['GWI_LOG_FILE']).parent)}) + '\\n')\n"
                    + ("sys.exit(0)\n" if leader_exits else "time.sleep(20)\n")
                )
                bad.chmod(0o755)
                good = self.binary()
                observed = []
                owned = []
                timeout_started = []
                original_run = runner.run_binary
                original_popen = runner.subprocess.Popen
                original_isolated = runner.run_isolated

                def isolated(arguments, **kwargs):
                    if arguments[0] == str(bad) and (('--list' in arguments) == (phase == 'list')):
                        kwargs['timeout'] = 0.5
                    return original_isolated(arguments, **kwargs)

                def popen(*args, **kwargs):
                    process = original_popen(*args, **kwargs)
                    if kwargs.get('start_new_session'):
                        owned.append(process)
                        arguments = args[0]
                        if arguments[0] == str(bad) and (('--list' in arguments) == (phase == 'list')):
                            # Start the short execution deadline only once the
                            # descendant is ready, independent of interpreter
                            # startup speed or concurrent Rust builds.
                            deadline = time.monotonic() + 10
                            while not records.exists() or not any(
                                    json.loads(line)['parent'] == process.pid
                                    for line in records.read_text().splitlines()):
                                self.assertLess(time.monotonic(), deadline, 'Fixture startup timed out')
                                time.sleep(0.01)
                            timeout_started.append(time.monotonic())
                    return process

                def run(binary, case):
                    # Check before the next invocation can touch any paths.
                    self.assertTrue(all(p.returncode is not None for p in owned))
                    if records.exists():
                        for line in records.read_text().splitlines():
                            record = json.loads(line)
                            self.assertNotEqual(record['group'], os.getpgrp())
                            self.assert_terminated(record['parent'])
                            self.assert_terminated(record['child'])
                            self.assertFalse(Path(record['scratch']).exists())
                    observed.append((binary, case))
                    result = original_run(binary, case)
                    self.assertEqual(result, binary == good)
                    if binary == bad:
                        self.assertLess(time.monotonic() - timeout_started[-1], 5)
                    return result

                try:
                    with patch.object(runner, 'TERMINATION_GRACE', 0.2), \
                            patch.object(runner, 'CLEANUP_TIMEOUT', 1), \
                            patch.object(runner, 'build_binaries', return_value=[bad, good]), \
                            patch.object(runner, 'run_binary', side_effect=run), \
                            patch.object(runner, 'run_isolated', side_effect=isolated), \
                            patch.object(runner.subprocess, 'Popen', side_effect=popen):
                        self.assertEqual(runner.main(), 1)
                    self.assertEqual(observed, [(b, c) for c in runner.CASES for b in (bad, good)])
                    self.assertEqual(len(records.read_text().splitlines()), 2)
                    self.assertTrue(owned)
                    self.assertTrue(all(process.returncode is not None for process in owned),
                                    ([(p.pid, p.args, p.returncode) for p in owned], self.output.getvalue()))
                    for case in runner.CASES:
                        self.assertIn(f'{case}: {bad}:', self.output.getvalue())
                    self.assertIn('timed out', self.output.getvalue())
                    self.assert_cleaned()
                finally:
                    # A broken implementation must not leak fixture processes.
                    if records.exists():
                        for line in records.read_text().splitlines():
                            record = json.loads(line)
                            for pid in (record['parent'], record['child']):
                                try:
                                    os.kill(pid, signal.SIGKILL)
                                except ProcessLookupError:
                                    pass
                    for process in owned:
                        if process.returncode is None:
                            runner.signal_group(process.pid, signal.SIGKILL)
                        process.wait(timeout=2)

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
