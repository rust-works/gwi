#!/usr/bin/env python3
"""Probe regressions with fake Cargo/libtest and real inherited descendants."""

import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("log_follow_probe", Path(__file__).with_name("log-follow-probe.py"))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


@unittest.skipUnless(sys.platform in ("darwin", "linux"), "POSIX sessions required")
class ProbeTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.probe = runner.Probe(self.root, 60)
        self.addCleanup(self.probe.cleanup)

    def binary(self, name, code):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(f"#!{sys.executable}\n" + code)
        path.chmod(0o755)
        return path

    def fake_test(self, name="test", status=0, passed=1, diagnostics=True):
        events = '\n'.join(f'{phase} at 1ms: Ok((1ms, Ok("record")))' for phase in runner.PHASES)
        return self.binary(name, f"""import sys
if '--list' in sys.argv:
    print({(runner.TEST + ': test')!r})
else:
    print({(events + chr(10) + events if diagnostics else '')!r}, file=sys.stderr)
    print('test result: ok. {passed} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out')
    sys.exit({status})
""")

    def test_build_selects_separate_feature_artifacts(self):
        for feature in ('default', 'mcp'):
            test = self.fake_test(f'build/{feature}/cli_test')
            cli = self.binary(f'build/{feature}/gwi', 'pass\n')
            artifacts = [
                {'reason': 'compiler-artifact', 'target': {'name': name, 'kind': [kind]},
                 'profile': {'test': kind == 'test'}, 'executable': str(binary)}
                for name, kind, binary in [('cli_test', 'test', test), ('gwi', 'bin', cli)]
            ]
            cargo = self.binary('cargo', f"print({chr(10).join(map(json.dumps, artifacts))!r})\n")
            with patch.dict(os.environ, {'PATH': str(self.root) + os.pathsep + os.environ['PATH']}):
                binaries = runner.build(self.probe, feature, 10)
            self.assertEqual(binaries, {'cli_test': test, 'gwi': cli})
            command = self.probe.runs[-2].record['command']
            self.assertIn(str(self.root / 'build' / feature), command)
            self.assertEqual('--features' in command, feature == 'mcp')
            self.assertTrue(cargo.exists())

    def test_missing_exact_listing_fails(self):
        test = self.fake_test('build/default/cli_test')
        test.write_text(test.read_text().replace(runner.TEST + ': test', 'other: test'))
        cli = self.binary('build/default/gwi', 'pass\n')
        artifacts = [
            {'reason': 'compiler-artifact', 'target': {'name': name, 'kind': [kind]},
             'profile': {'test': kind == 'test'}, 'executable': str(binary)}
            for name, kind, binary in [('cli_test', 'test', test), ('gwi', 'bin', cli)]
        ]
        self.binary('cargo', f"print({chr(10).join(map(json.dumps, artifacts))!r})\n")
        with patch.dict(os.environ, {'PATH': str(self.root) + os.pathsep + os.environ['PATH']}):
            with self.assertRaisesRegex(RuntimeError, 'Missing exact test'):
                runner.build(self.probe, 'default', 10)

    def test_full_harness_command_and_timeout_retention(self):
        test = self.fake_test()
        args = runner.arguments(['--output', str(self.root), '--repetitions', '1',
                                 '--workload', 'harness', '--concurrency', '1'])
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertTrue(runner.measure(self.probe, 'default', {'cli_test': test}, args,
                                           {'repetitions': []}))
        workloads = [r for r in self.probe.runs if r.record['role'] == 'harness']
        self.assertTrue(workloads)
        self.assertIn('--skip', workloads[0].record['command'])
        self.assertIn(runner.TEST, workloads[0].record['command'])
        test = self.binary('hung-test', 'import time\ntime.sleep(60)\n')
        args = runner.arguments(['--output', str(self.root), '--repetitions', '1', '--run-timeout', '0.1'])
        summary = {'repetitions': []}
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertFalse(runner.measure(self.probe, 'mcp', {'cli_test': test}, args, summary))
        self.assertEqual(summary['repetitions'][0]['outcome'], 'timeout')
        self.assertTrue((self.root / 'mcp-run-1/command.json').exists())

    def test_missing_artifacts_and_wrong_directory_fail(self):
        for index, artifacts in enumerate(([], [{
            'reason': 'compiler-artifact', 'target': {'name': 'cli_test', 'kind': ['test']},
            'profile': {'test': True}, 'executable': str(self.fake_test())
        }])):
            self.binary('cargo', f"print({chr(10).join(map(json.dumps, artifacts))!r})\n")
            with patch.dict(os.environ, {'PATH': str(self.root) + os.pathsep + os.environ['PATH']}):
                with self.assertRaisesRegex(RuntimeError, 'Missing|Invalid'):
                    runner.build(self.probe, f'case{index}', 10)

    def test_failure_is_retained_and_never_masked_by_later_pass(self):
        test = self.fake_test(status=7)
        args = runner.arguments(['--output', str(self.root), '--repetitions', '2'])
        summary = {'repetitions': []}
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertFalse(runner.measure(self.probe, 'default', {'cli_test': test}, args, summary))
        self.assertEqual([e['returncode'] for e in summary['repetitions']], [7, 7])
        self.assertEqual([e['invocation'] for e in summary['repetitions']], ['first-focused', 'subsequent-focused'])
        self.assertEqual(len(summary['repetitions'][0]['phases']), 6)
        self.assertIn('record', (self.root / 'default-run-1/stderr.log').read_text())
        # A later passing feature cannot erase the earlier failed feature result.
        test = self.fake_test('passing')
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertTrue(runner.measure(self.probe, 'mcp', {'cli_test': test}, args, summary))
        self.assertEqual(summary['repetitions'][0]['returncode'], 7)

    def test_zero_selection_and_missing_diagnostics_fail(self):
        for index, kwargs in enumerate(({'passed': 0}, {'passed': 2}, {'diagnostics': False})):
            test = self.fake_test(f'test{index}', **kwargs)
            args = runner.arguments(['--output', str(self.root), '--repetitions', '1'])
            summary = {'repetitions': []}
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertFalse(runner.measure(self.probe, f'case{index}', {'cli_test': test}, args, summary))
            self.assertEqual(summary['repetitions'][0]['outcome'], 'invalid-output')

    def test_startup_and_cpu_workload_commands_are_recorded_and_reaped(self):
        test = self.fake_test()
        cli = self.binary('gwi', 'import time\ntime.sleep(0.05)\n')
        args = runner.arguments(['--output', str(self.root), '--repetitions', '2', '--cpu-workers', '1',
                                 '--workload', 'startup', '--concurrency', '2'])
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertTrue(runner.measure(self.probe, 'default', {'cli_test': test, 'gwi': cli}, args,
                                           {'repetitions': []}))
        self.assertTrue(any(r.record['command'] == [str(cli), '--help'] for r in self.probe.runs))
        self.assertTrue(any(r.record['role'] == 'cpu' for r in self.probe.runs))
        for run in self.probe.runs:
            self.assertTrue(run.done)
            self.assertIsNotNone(run.child.returncode)

    def test_workload_failure_fails_probe(self):
        test = self.fake_test()
        # Give the workload time to exit before the focused fake test.
        content = test.read_text().replace("else:\n", "else:\n    import time; time.sleep(0.1)\n")
        test.write_text(content)
        cli = self.binary('gwi', 'import sys\nsys.exit(9)\n')
        args = runner.arguments(['--output', str(self.root), '--repetitions', '1',
                                 '--workload', 'startup', '--concurrency', '1'])
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertFalse(runner.measure(self.probe, 'default', {'cli_test': test, 'gwi': cli}, args,
                                            {'repetitions': []}))
        self.assertTrue(any(r.record.get('returncode') == 9 for r in self.probe.runs))

    def descendant(self, immediate_exit=False):
        pidfile = self.root / 'descendant.pid'
        pidfile.unlink(missing_ok=True)
        child_code = (
            "import os,pathlib,signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); "
            f"p=pathlib.Path({str(pidfile)!r}); t=p.with_suffix('.tmp'); "
            "t.write_text(str(os.getpid())); t.replace(p); time.sleep(60)"
        )
        parent = self.binary('parent', f"""import pathlib, subprocess, sys, time
child = subprocess.Popen([sys.executable, '-c', {child_code!r}])
time.sleep({0.1 if immediate_exit else 60})
""")
        return parent, pidfile

    def assert_not_running(self, pid):
        # Orphan zombies can await the host's init reaper; they cannot execute or
        # retain handles. ps handles this on both supported platforms.
        result = subprocess.run(['ps', '-o', 'stat=', '-p', str(pid)], capture_output=True, text=True)
        self.assertTrue(not result.stdout.strip() or result.stdout.lstrip().startswith('Z'), result.stdout)

    def test_timeout_and_success_clean_descendants_ignoring_term(self):
        for immediate in (False, True):
            parent, pidfile = self.descendant(immediate)
            run = self.probe.start(f'parent-{immediate}', [str(parent)], 'focused', 10)
            deadline = time.monotonic() + 5
            while not pidfile.exists() and time.monotonic() < deadline:
                time.sleep(0.01)
            self.assertTrue(pidfile.exists())
            if not immediate:
                run.timeout = time.monotonic() - run.started + 0.4
            result = self.probe.wait(run)
            self.assertEqual(result['outcome'], 'passed' if immediate else 'timeout')
            self.assertLess(result['elapsed_seconds'], 7)
            self.assert_not_running(int(pidfile.read_text()))
            self.assertIsNotNone(run.child.returncode)

    def test_interruption_and_total_deadline_cleanup(self):
        for cause in (KeyboardInterrupt, RuntimeError):
            parent, pidfile = self.descendant()
            run = self.probe.start(f'interrupt-{cause.__name__}', [str(parent)], 'focused', 60)
            deadline = time.monotonic() + 5
            while not pidfile.exists() and time.monotonic() < deadline:
                time.sleep(0.01)
            with patch.object(self.probe, 'check_deadline', side_effect=cause('stop')):
                with self.assertRaises(cause):
                    try:
                        self.probe.wait(run)
                    finally:
                        self.probe.cleanup()
            self.assert_not_running(int(pidfile.read_text()))
            pidfile.unlink()

    def cleanup_runner(self, child, output, pidfile):
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                pass
        groups = set()
        for path in output.glob('*/command.json'):
            record = json.loads(path.read_text())
            if 'outcome' not in record and 'pid' in record:
                groups.add(record['pid'])
        if pidfile.exists():
            try:
                groups.add(os.getpgid(int(pidfile.read_text())))
            except ProcessLookupError:
                pass
        try:
            for group in groups:
                try:
                    os.killpg(group, signal.SIGKILL)
                except (ProcessLookupError, PermissionError):
                    # Confirm exit below; a denied signal cannot hide a live group.
                    pass
        finally:
            if child.poll() is None:
                child.kill()
            child.wait(timeout=3)
        deadline = time.monotonic() + 3
        while groups:
            snapshot = subprocess.run(['ps', '-axo', 'pgid=,stat='], capture_output=True,
                                      text=True, check=True, timeout=5)
            live = {int(group) for group, state in (line.split() for line in snapshot.stdout.splitlines())
                    if not state.startswith('Z')}
            groups &= live
            if time.monotonic() >= deadline:
                self.assertFalse(groups, 'signal fixture left live workload groups')
            if groups:
                time.sleep(0.01)
        pidfile.unlink(missing_ok=True)

    def test_signal_fixture_cleanup_handles_a_crashed_probe(self):
        parent, pidfile = self.descendant()
        output = self.root / 'crashed'
        command_dir = output / 'build'
        command_dir.mkdir(parents=True)
        broken = self.binary('broken-probe', f"""import json,pathlib,subprocess,time
child = subprocess.Popen([{str(parent)!r}], start_new_session=True)
pathlib.Path({str(command_dir / 'command.json')!r}).write_text(json.dumps({{'pid': child.pid}}))
time.sleep(60)
""")
        child = subprocess.Popen([str(broken)])
        try:
            deadline = time.monotonic() + 10
            while not pidfile.exists() and time.monotonic() < deadline:
                time.sleep(0.01)
            self.assertTrue(pidfile.exists())
            descendant = int(pidfile.read_text())
            child.kill()
            child.wait(timeout=3)
            self.cleanup_runner(child, output, pidfile)
            self.assert_not_running(descendant)
        finally:
            self.cleanup_runner(child, output, pidfile)

    def test_real_signals_cleanup_and_preserve_summary(self):
        # Exercise main's installed signal handlers while a real build descendant
        # is running, rather than only injecting an exception into wait().
        for sig in (signal.SIGINT, signal.SIGTERM):
            parent, pidfile = self.descendant()
            self.binary('cargo', parent.read_text().split('\n', 1)[1])
            output = self.root / f'signal-{sig}'
            env = {**os.environ, 'PATH': str(self.root) + os.pathsep + os.environ['PATH']}
            child = subprocess.Popen([sys.executable, str(Path(runner.__file__)), '--output', str(output)],
                                     env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            try:
                deadline = time.monotonic() + 10
                while not pidfile.exists() and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue(pidfile.exists())
                child.send_signal(sig)
                self.assertEqual(child.wait(timeout=10), 130)
                self.assert_not_running(int(pidfile.read_text()))
                summary = json.loads((output / 'summary.json').read_text())
                self.assertEqual(summary['status'], 130)
                self.assertEqual(summary['commands'][-1]['outcome'], 'stopped')
            finally:
                self.cleanup_runner(child, output, pidfile)

    def test_metadata_write_failure_keeps_child_owned_for_cleanup(self):
        with patch.object(runner, 'write_json', side_effect=OSError('disk full')):
            with self.assertRaisesRegex(OSError, 'disk full'):
                self.probe.start('write-failure', [sys.executable, '-c', 'import time; time.sleep(60)'],
                                 'focused', 60)
        self.probe.cleanup()
        self.assertEqual(len(self.probe.runs), 1)
        self.assertTrue(self.probe.runs[0].done)
        self.assertIsNotNone(self.probe.runs[0].child.returncode)

    def test_permission_denial_is_suppressed_only_for_absent_live_group(self):
        run = self.probe.start('exited', [sys.executable, '-c', 'pass'], 'focused', 10)
        run.child.wait(timeout=10)
        with patch.object(runner.os, 'killpg', side_effect=PermissionError('denied')):
            self.assertEqual(run.finish('passed')['outcome'], 'passed')
        self.assertTrue(run.record['cleanup_notes'])
        live = self.probe.start('live', [sys.executable, '-c', 'import time; time.sleep(60)'], 'cpu', 60)
        with patch.object(runner.os, 'killpg', side_effect=PermissionError('denied')):
            with self.assertRaises(PermissionError):
                live.finish('stopped')
        self.probe.cleanup()
        self.assertTrue(live.done)

    def test_cleanup_continues_after_one_group_error(self):
        first = self.probe.start('first', [sys.executable, '-c', 'import time; time.sleep(60)'], 'cpu', 60)
        second = self.probe.start('second', [sys.executable, '-c', 'import time; time.sleep(60)'], 'cpu', 60)
        with patch.object(first, 'finish', side_effect=PermissionError('denied')):
            with self.assertRaisesRegex(RuntimeError, 'Cleanup failed: first'):
                self.probe.cleanup()
        self.assertTrue(second.done)
        self.probe.cleanup()
        self.assertTrue(first.done)

    def test_failed_spawn_retains_command_status_and_output_files(self):
        with self.assertRaises(OSError):
            self.probe.start('missing', [str(self.root / 'absent')], 'focused', 10)
        record = json.loads((self.root / 'missing/command.json').read_text())
        self.assertEqual(record['outcome'], 'launch-failed')
        self.assertIsNone(record['returncode'])
        self.assertTrue((self.root / 'missing/stderr.log').exists())

    def test_argument_bounds(self):
        for options in (['--run-timeout', 'nan'], ['--max-seconds', 'inf'], ['--repetitions', '0'],
                        ['--cpu-workers', '-1'], ['--workload', 'startup'], ['--concurrency', '1']):
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                runner.arguments(['--output', str(self.root), *options])


if __name__ == '__main__':
    unittest.main()
