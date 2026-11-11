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

    def artifacts(self, feature, entries):
        lines = [json.dumps({'reason': 'compiler-artifact', 'target': {'name': name, 'kind': [kind]},
                             'profile': {'test': is_test}, 'executable': str(binary)})
                 for name, kind, is_test, binary in entries]
        self.binary('cargo', f"print({chr(10).join(lines)!r})\n")

    def test_lib_build_selects_library_test_executable_only_when_requested(self):
        test = self.fake_test('build/default/cli_test')
        cli = self.binary('build/default/gwi', 'pass\n')
        lib = self.binary('build/default/lib_test', 'pass\n')
        self.artifacts('default', [('cli_test', 'test', True, test), ('gwi', 'bin', False, cli),
                                   ('gwi', 'lib', True, lib)])
        with patch.dict(os.environ, {'PATH': str(self.root) + os.pathsep + os.environ['PATH']}):
            binaries = runner.build(self.probe, 'default', 10, lib=True)
            self.assertEqual(binaries, {'cli_test': test, 'gwi': cli, 'lib_test': lib})
            self.assertIn('--lib', self.probe.runs[-2].record['command'])

    def test_build_without_lib_request_omits_library_target(self):
        test = self.fake_test('build/default/cli_test')
        cli = self.binary('build/default/gwi', 'pass\n')
        lib = self.binary('build/default/lib_test', 'pass\n')
        self.artifacts('default', [('cli_test', 'test', True, test), ('gwi', 'bin', False, cli),
                                   ('gwi', 'lib', True, lib)])
        with patch.dict(os.environ, {'PATH': str(self.root) + os.pathsep + os.environ['PATH']}):
            plain = runner.build(self.probe, 'default', 10)
        self.assertEqual(set(plain), {'cli_test', 'gwi'})
        self.assertNotIn('--lib', self.probe.runs[-2].record['command'])

    def test_ambiguous_library_executables_fail(self):
        test = self.fake_test('build/default/cli_test')
        cli = self.binary('build/default/gwi', 'pass\n')
        first = self.binary('build/default/lib_a', 'pass\n')
        second = self.binary('build/default/lib_b', 'pass\n')
        self.artifacts('default', [('cli_test', 'test', True, test), ('gwi', 'bin', False, cli),
                                   ('gwi', 'lib', True, first), ('gwi', 'lib', True, second)])
        with patch.dict(os.environ, {'PATH': str(self.root) + os.pathsep + os.environ['PATH']}):
            with self.assertRaisesRegex(RuntimeError, 'Ambiguous default library test'):
                runner.build(self.probe, 'default', 10, lib=True)

    def test_build_load_uses_the_build_timeout_not_the_workload_timeout(self):
        self.binary('cargo', 'import time\ntime.sleep(60)\n')
        test = self.fake_test()
        content = test.read_text().replace("else:\n", "else:\n    import time; time.sleep(0.5)\n")
        test.write_text(content)
        args = runner.arguments(['--output', str(self.root), '--repetitions', '1', '--build-slots', '1',
                                 '--workload-timeout', '0.05', '--build-timeout', '60'])
        with patch.dict(os.environ, {'PATH': str(self.root) + os.pathsep + os.environ['PATH']}):
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertTrue(runner.measure(self.probe, 'default', {'cli_test': test}, args,
                                               {'repetitions': []}))
        slot = [r for r in self.probe.runs if r.record['role'] == 'build-load']
        self.assertEqual(len(slot), 1)
        self.assertEqual(slot[0].record['timeout_seconds'], 60)

    def test_lib_build_without_library_executable_fails(self):
        test = self.fake_test('build/default/cli_test')
        cli = self.binary('build/default/gwi', 'pass\n')
        self.artifacts('default', [('cli_test', 'test', True, test), ('gwi', 'bin', False, cli)])
        with patch.dict(os.environ, {'PATH': str(self.root) + os.pathsep + os.environ['PATH']}):
            with self.assertRaisesRegex(RuntimeError, 'Missing default Cargo executables'):
                runner.build(self.probe, 'default', 10, lib=True)

    def test_lib_workload_records_overlap_and_restarts_with_summary_check(self):
        test = self.fake_test()
        content = test.read_text().replace("else:\n", "else:\n    import time; time.sleep(0.3)\n")
        test.write_text(content)
        lib = self.binary('lib_test', "print('test result: ok. 5 passed; 0 failed; 0 ignored')\n")
        args = runner.arguments(['--output', str(self.root), '--repetitions', '1', '--workload', 'lib',
                                 '--concurrency', '2', '--lib-threads', '3'])
        summary = {'repetitions': []}
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertTrue(runner.measure(self.probe, 'default', {'cli_test': test, 'lib_test': lib}, args,
                                           summary))
        loads = [r for r in self.probe.runs if r.record['role'] == 'lib']
        self.assertGreater(len(loads), 2)  # restarted after exit
        self.assertEqual(loads[0].record['command'], [str(lib), '--test-threads', '3', '--color', 'never'])
        overlap = summary['repetitions'][0]['overlap']
        self.assertEqual(len(overlap['live_at_start']), 2)
        self.assertTrue(overlap['commands'])
        self.assertTrue(all(c['role'] == 'lib' and c['overlap_seconds'] > 0 for c in overlap['commands']))
        self.assertEqual(len(overlap['loadavg_start']), 3)
        self.assertEqual(len(overlap['loadavg_end']), 3)
        self.assertTrue(all(r.record.get('ended_utc') for r in self.probe.runs))

    def test_lib_workload_without_passing_summary_fails(self):
        test = self.fake_test()
        content = test.read_text().replace("else:\n", "else:\n    import time; time.sleep(1.5)\n")
        test.write_text(content)
        lib = self.binary('lib_test', "print('test result: ok. 0 passed; 0 failed')\n")
        args = runner.arguments(['--output', str(self.root), '--repetitions', '1', '--workload', 'lib',
                                 '--concurrency', '1'])
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertFalse(runner.measure(self.probe, 'default', {'cli_test': test, 'lib_test': lib}, args,
                                            {'repetitions': []}))

    def test_overlap_excludes_loads_that_ended_before_the_focused_run(self):
        class Load:
            def __init__(self, name, started, ended):
                self.directory, self.started, self.ended = Path(name), started, ended
                self.record = {'role': 'lib', 'started_utc': 'u', 'ended_utc': None if ended is None else 'e'}
        focused = type('F', (), {'started': 10.0, 'ended': 20.0})()
        loads = [Load('before', 1, 9), Load('spans', 1, None), Load('inside', 12, 15), Load('after', 21, None)]
        record = runner.overlap_record(loads, focused, [0, 0, 0], [1, 1, 1])
        self.assertEqual([c['directory'] for c in record['commands']], ['spans', 'inside'])
        self.assertEqual(record['live_at_start'], ['spans'])
        self.assertEqual(record['live_at_end'], ['spans'])
        self.assertEqual(record['commands'][1]['overlap_seconds'], 3)

    def test_build_load_slots_are_owned_warmed_and_reaped_with_descendants(self):
        for feature in ('default', 'mcp'):
            self.binary('cargo', "import sys\nprint(' '.join(sys.argv[1:]))\n")
            args = runner.arguments(['--output', str(self.root), '--build-slots', '2', '--build-jobs', '3'])
            with patch.dict(os.environ, {'PATH': str(self.root) + os.pathsep + os.environ['PATH']}):
                runner.warm_build_load(self.probe, feature, args)
            warm = [r for r in self.probe.runs if r.record['role'] == 'build-warm'][-2:]
            self.assertEqual([r.record['outcome'] for r in warm], ['passed', 'passed'])
            commands = [r.record['command'] for r in warm]
            self.assertEqual(len({c[c.index('--target-dir') + 1] for c in commands}), 2)
            self.assertTrue(all(c[:5] == ['cargo', 'build', '--locked', '--jobs', '3'] for c in commands))
            self.assertEqual(all('--features' in c for c in commands), feature == 'mcp')
        # A TERM-ignoring cargo descendant must die with its slot's session.
        pidfile = self.root / 'build-load.pid'
        self.binary('cargo', f"""import os, pathlib, signal, time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
pathlib.Path({str(pidfile)!r}).write_text(str(os.getpid()))
time.sleep(60)
""")
        test = self.fake_test()
        content = test.read_text().replace("else:\n", "else:\n    import time; time.sleep(0.5)\n")
        test.write_text(content)
        args = runner.arguments(['--output', str(self.root), '--repetitions', '1', '--build-slots', '1'])
        summary = {'repetitions': []}
        with patch.dict(os.environ, {'PATH': str(self.root) + os.pathsep + os.environ['PATH']}):
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertTrue(runner.measure(self.probe, 'mcp', {'cli_test': test}, args, summary))
        slot = [r for r in self.probe.runs if r.record['role'] == 'build-load'][0]
        self.assertTrue(slot.done)
        self.assertEqual(slot.record['outcome'], 'stopped')
        self.assertIn('mcp', slot.record['command'])
        self.assertIn(str(self.root / 'overlap-build' / 'mcp-0'), slot.record['command'])
        self.assertTrue(pidfile.exists())
        self.assert_not_running(int(pidfile.read_text()))
        self.assertEqual([c['role'] for c in summary['repetitions'][0]['overlap']['commands']], ['build-load'])

    def test_failed_build_load_fails_probe(self):
        self.binary('cargo', 'import sys\nsys.exit(7)\n')
        test = self.fake_test()
        content = test.read_text().replace("else:\n", "else:\n    import time; time.sleep(1.5)\n")
        test.write_text(content)
        args = runner.arguments(['--output', str(self.root), '--repetitions', '1', '--build-slots', '1'])
        with patch.dict(os.environ, {'PATH': str(self.root) + os.pathsep + os.environ['PATH']}):
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertFalse(runner.measure(self.probe, 'default', {'cli_test': test}, args,
                                                {'repetitions': []}))

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

    def test_probe_configuration_is_scoped_to_focused_capture(self):
        test = self.fake_test('env-test')
        code = test.read_text().replace('import sys\n', "import sys, os\nif '--list' not in sys.argv:\n    focused = '--exact' in sys.argv\n    assert ('GWI_FOLLOW_PROBE_DIR' in os.environ) == focused\n    if focused:\n        assert os.environ['GWI_FOLLOW_PROBE_DELAY'] == '0'\n    else:\n        assert 'GWI_FOLLOW_PROBE_DELAY' not in os.environ\n")
        test.write_text(code)
        args = runner.arguments(['--output', str(self.root), '--repetitions', '1', '--capture-live',
                                 '--workload', 'harness', '--concurrency', '1'])
        marker = self.root / 'workload-env-checked'
        readiness = f"""    import pathlib, time
    marker = pathlib.Path({str(marker)!r})
    if focused:
        deadline = time.monotonic() + 5
        while not marker.exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        assert marker.exists()
    else:
        marker.write_text('checked')
"""
        test.write_text(test.read_text().replace("if '--list' in sys.argv:", readiness + "if '--list' in sys.argv:"))
        summary = {'repetitions': []}
        with patch.dict(os.environ, {'GWI_FOLLOW_PROBE_DIR': '/absent', 'GWI_FOLLOW_PROBE_DELAY': '99'}), \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertTrue(runner.measure(self.probe, 'default', {'cli_test': test}, args, summary))
        self.assertEqual(summary['repetitions'][0]['capture_evidence'], 'missing-fixture-state')
        self.assertEqual(marker.read_text(), 'checked')

    def live_fixture(self, name='live-fixture'):
        directory = self.root / name
        fixture = self.binary(name + '-binary', f"""import json, os, pathlib, subprocess, sys, time
child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])
state = {{'pid': child.pid, 'harness_pid': os.getpid(), 'phase': {runner.PHASES[0]!r},
         'status': 'waiting', 'receive_started_unix_seconds': time.time(),
         'receive_timeout_seconds': 10}}
pathlib.Path({str(directory)!r}, 'follower-' + str(child.pid) + '.json').write_text(json.dumps(state))
time.sleep(60)
""")
        run = self.probe.start(name, [str(fixture)], 'focused', 60)
        deadline = time.monotonic() + 5
        while not list(directory.glob('follower-*.json')) and time.monotonic() < deadline:
            time.sleep(0.01)
        paths = list(directory.glob('follower-*.json'))
        self.assertEqual(len(paths), 1)
        state = json.loads(paths[0].read_text())
        args = runner.arguments(['--output', str(self.root), '--capture-live',
                                 '--capture-after', '0.01', '--capture-timeout', '2'])
        return run, paths[0], state, runner.LiveCapture(self.probe, run, args)

    def drain_capture(self, capture):
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            capture.tick()
            if capture.records and not capture.active:
                return
            time.sleep(0.01)
        self.fail('capture did not finish within test bound')

    def test_capture_targets_live_follower_and_retains_raw_results(self):
        run, path, state, capture = self.live_fixture()
        tool = self.binary('capture-tool', '')
        tool.write_text('#!/bin/sh\nprintf "target %s\\n" "$1"\n')
        with patch.object(capture, 'command', side_effect=lambda pid, stage: [str(tool), str(pid)]), \
                patch.object(runner.sys, 'platform', 'darwin'):
            time.sleep(0.02)
            self.drain_capture(capture)
        record = capture.records[state['pid']]
        self.assertEqual(record['outcome'], 'passed')
        self.assertEqual(len(record['results']), 2)
        self.assertEqual(record['fixture_at_capture']['harness_pid'], run.child.pid)
        self.assertNotEqual(state['pid'], run.child.pid)
        self.assertIsNone(run.child.poll())
        for command in record['commands']:
            self.assertIn(str(state['pid']), (self.root / command / 'stdout.log').read_text())
        capture.finish()
        self.probe.cleanup()
        self.assert_not_running(state['pid'])

    def test_capture_failures_unavailable_and_hung_are_bounded(self):
        for name, code, outcome in (
                ('failed', 'import sys; sys.exit(7)\n', 'failed'),
                ('missing', None, 'unavailable'),
                ('hung', 'import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(60)\n', 'timeout')):
            run, path, state, capture = self.live_fixture(name)
            tool = self.root / 'absent' if code is None else self.binary('tool-' + name, code)
            if name == 'failed':
                tool.write_text('#!/bin/sh\nexit 7\n')
            started = time.monotonic()
            with patch.object(capture, 'command', return_value=[str(tool)]):
                time.sleep(0.02)
                self.drain_capture(capture)
            record = capture.records[state['pid']]
            self.assertEqual(record['outcome'], outcome)
            self.assertLess(time.monotonic() - started, 4)
            self.assertIsNone(run.child.poll(), 'capture must not kill the follower')
            if name == 'failed':
                self.assertEqual(record['results'][0]['returncode'], 7)
            if name == 'missing':
                self.assertEqual(json.loads((self.root / record['commands'][0] / 'command.json').read_text())['outcome'],
                                 'launch-failed')
            capture.finish()
            self.probe.cleanup()
            self.assert_not_running(state['pid'])

    def test_sample_shares_process_budget_and_revalidates_phase(self):
        for changed in (False, True):
            run, path, state, capture = self.live_fixture('shared-' + str(changed))
            process = self.binary('slow-process-' + str(changed), '')
            process.write_text('#!/bin/sh\nsleep 0.15\n')
            sample = self.binary('hung-sample-' + str(changed), 'import time; time.sleep(60)\n')
            started = time.monotonic()
            with patch.object(runner.sys, 'platform', 'darwin'), \
                    patch.object(capture, 'command', side_effect=lambda pid, stage:
                                 [str(process if stage == 'process' else sample)]):
                time.sleep(0.02)
                capture.tick()
                if changed:
                    state['status'] = 'received'
                    path.write_text(json.dumps(state))
                self.drain_capture(capture)
            record = capture.records[state['pid']]
            self.assertEqual(record['outcome'], 'late-or-exited' if changed else 'timeout')
            self.assertLess(time.monotonic() - started, 4)
            self.assertEqual(len(record['commands']), 1 if changed else 2)
            if not changed:
                self.assertLess(record['results'][1]['timeout_seconds'], 1.9)
            capture.finish()
        self.probe.cleanup()

    def test_capture_watchdog_expires_without_supervisor_polling(self):
        # A slow workload cleanup must not extend a running capture command.
        run = self.probe.start('unpolled-capture', ['/bin/sh', '-c', 'exec sleep 60'], 'capture', 0.2)
        time.sleep(0.5)
        self.assert_not_running(run.child.pid)
        self.assertEqual(run.poll()['outcome'], 'timeout')
        self.assertIsNone(run.capture_timer)
        live = self.probe.start('cancelled-capture', ['/bin/sh', '-c', 'exec sleep 60'], 'capture', 30)
        timer = live.capture_timer
        self.probe.cleanup()
        self.assertFalse(timer.is_alive())
        self.assert_not_running(live.child.pid)

    def test_watchdog_start_failure_still_reaps_owned_capture(self):
        with patch.object(runner.threading.Timer, 'start', side_effect=RuntimeError('no threads')):
            with self.assertRaisesRegex(RuntimeError, 'no threads'):
                self.probe.start('thread-failure', ['/bin/sh', '-c', 'exec sleep 60'], 'capture', 2)
        self.probe.cleanup()
        self.assertTrue(self.probe.runs[0].done)
        self.assert_not_running(self.probe.runs[0].child.pid)

    def test_finalization_preserves_already_expired_capture(self):
        run, path, state, capture = self.live_fixture('expired-finalization')
        capture.args.capture_timeout = 0.1
        with patch.object(capture, 'command', return_value=['/bin/sh', '-c', 'exec sleep 60']):
            time.sleep(0.02)
            capture.tick()
        time.sleep(0.3)
        capture.finish()
        record = capture.records[state['pid']]
        self.assertEqual(record['outcome'], 'timeout')
        self.assertTrue(record['results'][0]['capture_watchdog_expired'])
        self.probe.cleanup()
        self.assert_not_running(state['pid'])

    def test_capture_gates_phase_identity_and_remaining_budget(self):
        for index, changes, expected in (
                (0, {'status': 'received'}, 'not-delayed-or-missed'),
                (1, {'phase': runner.PHASES[1]}, 'not-delayed-or-missed'),
                (2, {'receive_started_unix_seconds': time.time() - 20}, 'budget-exhausted'),
                (3, {'pid': os.getpid()}, 'late-or-exited')):
            run, path, state, capture = self.live_fixture('gate-' + str(index))
            state.update(changes)
            target = path.with_name('follower-' + str(state['pid']) + '.json')
            path.unlink()
            target.write_text(json.dumps(state))
            with patch.object(capture, 'command') as command:
                time.sleep(0.02)
                capture.tick()
                command.assert_not_called()
            self.assertEqual(capture.records[state['pid']]['outcome'], expected)
        self.probe.cleanup()

    def test_interruption_during_capture_reaps_command_and_follower(self):
        # Exercise the actual runner signal handlers and main finally block while
        # a capture tool owns a TERM-ignoring descendant.
        for sig in (signal.SIGINT, signal.SIGTERM):
            output = self.root / f'capture-signal-{sig}'
            pidfile = self.root / f'capture-worker-{sig}.pid'
            fixture = self.binary(f'signal-test-{sig}', """import json,os,pathlib,subprocess,sys,time
if '--list' in sys.argv:
    print(""" + repr(runner.TEST + ': test') + """ )
else:
    child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])
    directory = pathlib.Path(os.environ['GWI_FOLLOW_PROBE_DIR'])
    state = {'pid': child.pid, 'harness_pid': os.getpid(), 'phase': 'initial backlog stdout',
             'status': 'waiting', 'receive_started_unix_seconds': time.time(), 'receive_timeout_seconds': 10}
    (directory / ('follower-' + str(child.pid) + '.json')).write_text(json.dumps(state))
    time.sleep(60)
""")
            tool = self.binary(f'signal-tool-{sig}', f"""import os,pathlib,signal,subprocess,sys,time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
child = subprocess.Popen([sys.executable, '-c', 'import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(60)'])
pathlib.Path({str(pidfile)!r}).write_text(str(child.pid))
time.sleep(60)
""")
            driver = self.binary(f'driver-{sig}', f"""import importlib.util,pathlib,shutil,sys
spec=importlib.util.spec_from_file_location('runner', {runner.__file__!r})
r=importlib.util.module_from_spec(spec); spec.loader.exec_module(r)
def build(probe, feature, timeout, lib=False):
    return {{'cli_test': pathlib.Path({str(fixture)!r}), 'gwi': pathlib.Path({str(fixture)!r})}}
r.build=build
r.LiveCapture.command=lambda self,pid,stage: [{str(tool)!r}]
sys.exit(r.main(['--output', {str(output)!r}, '--features', 'default', '--repetitions', '1',
                 '--capture-live', '--capture-after', '0.01', '--capture-timeout', '4']))
""")
            child = subprocess.Popen([str(driver)], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            try:
                deadline = time.monotonic() + 10
                while not pidfile.exists() and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue(pidfile.exists())
                child.send_signal(sig)
                _, stderr = child.communicate(timeout=10)
                self.assertEqual(child.returncode, 130, stderr.decode())
                self.assert_not_running(int(pidfile.read_text()))
                summary = json.loads((output / 'summary.json').read_text())
                self.assertEqual(summary['status'], 130)
                captures = json.loads((output / 'default-run-1/captures.json').read_text())
                self.assertEqual(captures[0]['outcome'], 'stopped')
                self.assert_not_running(captures[0]['pid'])
                for command in summary['commands']:
                    self.assert_not_running(command['pid'])
            finally:
                self.cleanup_runner(child, output, pidfile)

    def test_argument_bounds(self):
        for options in (['--run-timeout', 'nan'], ['--max-seconds', 'inf'], ['--repetitions', '0'],
                        ['--cpu-workers', '-1'], ['--workload', 'startup'], ['--concurrency', '1'],
                        ['--synthetic-delay', '1'], ['--capture-live', '--synthetic-delay', 'nan'],
                        ['--capture-after', '7'], ['--capture-timeout', '7'],
                        ['--lib-threads', '0'], ['--build-jobs', '0'], ['--build-slots', '-1'],
                        ['--workload', 'lib']):
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                runner.arguments(['--output', str(self.root), *options])


if __name__ == '__main__':
    unittest.main()
