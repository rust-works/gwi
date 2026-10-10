#!/usr/bin/env python3
"""Opt-in, bounded log-follow investigation; preserves every command's evidence."""

import argparse
import json
import math
import os
from pathlib import Path
import platform
import re
import signal
import subprocess
import sys
import time
from datetime import datetime, timezone

ROOT = Path(__file__).resolve().parents[1]
TEST = "log_follow_recovers_appended_records_and_warns_on_stderr"
PHASES = ("initial backlog stdout", "recovered appended stdout", "malformed-line stderr")
BURNER = "while True:\n    sum(i * i for i in range(10000))\n"
GRACE = 0.3
EXIT_GRACE = 2
INTERRUPTED = None


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


class CommandRun:
    """Own one session and its files, including descendants after parent exit."""

    def __init__(self, directory, command, role, timeout, env=None):
        directory.mkdir()
        self.directory = directory
        self.started = time.monotonic()
        self.timeout = timeout
        self.files = [(directory / name).open("wb") for name in ("stdout.log", "stderr.log")]
        self.record = {"command": list(map(str, command)), "role": role,
                       "started_utc": datetime.now(timezone.utc).isoformat(), "timeout_seconds": timeout,
                       "cwd": str(ROOT)}
        self.done = False
        try:
            self.child = subprocess.Popen(command, cwd=ROOT, env=env, start_new_session=True,
                                          stdout=self.files[0], stderr=self.files[1])
        except OSError as error:
            for stream in self.files:
                stream.close()
            self.record.update(outcome="launch-failed", returncode=None, error=str(error),
                               elapsed_seconds=time.monotonic() - self.started)
            write_json(directory / "command.json", self.record)
            raise
        self.record["pid"] = self.child.pid

    def group_states(self):
        snapshot = subprocess.run(["ps", "-axo", "pgid=,stat="], capture_output=True,
                                  text=True, check=True, timeout=5)
        return [state for group, state in (line.split(maxsplit=1) for line in snapshot.stdout.splitlines())
                if int(group) == self.child.pid and not state.startswith("Z")]

    def wait_for_group_exit(self):
        deadline = time.monotonic() + EXIT_GRACE
        while True:
            try:
                os.killpg(self.child.pid, 0)
            except ProcessLookupError:
                return
            except PermissionError:
                pass
            if not self.group_states():
                return
            if time.monotonic() >= deadline:
                raise RuntimeError(f"Live group remains after cleanup: {self.child.pid}")
            time.sleep(0.01)

    def signal_group(self, sig):
        try:
            os.killpg(self.child.pid, sig)
        except ProcessLookupError:
            pass
        except PermissionError:
            # A macOS group can be denied while its last member is exiting.
            # Wait for an observed exit; never suppress denial for a live worker.
            states = self.group_states()
            if states and not all("E" in state for state in states):
                raise
            self.wait_for_group_exit()
            self.record.setdefault("cleanup_notes", []).append(
                f"Signal {sig} denied; inspection confirmed no live group members")

    def finish(self, outcome):
        if self.done:
            return self.record
        # Always signal the group, even if the direct child has already exited.
        # A follower may still own resources or output handles in that session.
        self.signal_group(signal.SIGTERM)
        deadline = time.monotonic() + GRACE
        while time.monotonic() < deadline:
            self.child.poll()
            try:
                os.killpg(self.child.pid, 0)
            except ProcessLookupError:
                break
            except PermissionError:
                self.signal_group(0)
                break
            time.sleep(0.01)
        self.signal_group(signal.SIGKILL)
        status = self.child.wait()
        self.wait_for_group_exit()
        for stream in self.files:
            stream.close()
        self.record.update(outcome=outcome, returncode=status,
                           elapsed_seconds=time.monotonic() - self.started)
        self.done = True
        write_json(self.directory / "command.json", self.record)
        return self.record

    def poll(self):
        status = self.child.poll()
        if status is not None:
            return self.finish("passed" if status == 0 else "failed")
        if time.monotonic() - self.started >= self.timeout:
            return self.finish("timeout")
        return None

    def output(self):
        return "\n".join((self.directory / name).read_text(errors="replace")
                         for name in ("stdout.log", "stderr.log"))


class Probe:
    def __init__(self, output, max_seconds):
        self.output = output
        self.deadline = time.monotonic() + max_seconds
        self.runs = []

    def check_deadline(self):
        if INTERRUPTED is not None:
            raise KeyboardInterrupt(f"signal {INTERRUPTED}")
        if time.monotonic() >= self.deadline:
            raise RuntimeError("Total probe deadline exceeded")

    def start(self, name, command, role, timeout, env=None):
        self.check_deadline()
        run = CommandRun(self.output / name, command, role, timeout, env)
        self.runs.append(run)
        # Register ownership before writing metadata: an I/O failure must still
        # leave the spawned session reachable by cleanup.
        write_json(run.directory / "command.json", run.record)
        return run

    def wait(self, run):
        while True:
            self.check_deadline()
            result = run.poll()
            if result is not None:
                return result
            time.sleep(0.05)

    def cleanup(self):
        errors = []
        for run in self.runs:
            if not run.done:
                try:
                    run.finish("stopped")
                except (OSError, RuntimeError, subprocess.SubprocessError) as error:
                    run.record["cleanup_error"] = str(error)
                    errors.append(f"{run.directory.name}: {error}")
        if errors:
            raise RuntimeError("Cleanup failed: " + "; ".join(errors))


def require_pass(run, result):
    if result["outcome"] != "passed":
        raise RuntimeError(f"{run.directory.name}: {result['outcome']}; see {run.directory}")


def build(probe, feature, timeout):
    target = probe.output / "build" / feature
    command = ["cargo", "test", "--manifest-path", str(ROOT / "Cargo.toml"),
               "--target-dir", str(target), "--test", "cli_test", "--no-run", "--message-format=json"]
    if feature == "mcp":
        command += ["--features", "mcp"]
    run = probe.start(f"build-{feature}", command, "build", timeout)
    require_pass(run, probe.wait(run))
    binaries = {}
    for line in (run.directory / "stdout.log").read_text().splitlines():
        if not line.strip():
            continue
        artifact = json.loads(line)
        if artifact.get("reason") != "compiler-artifact" or not artifact.get("executable"):
            continue
        name = artifact["target"]["name"]
        kind = artifact["target"]["kind"]
        if (name == "cli_test" and "test" in kind and artifact["profile"]["test"]
                or name == "gwi" and "bin" in kind and not artifact["profile"]["test"]):
            binary = Path(artifact["executable"]).resolve()
            if not binary.is_relative_to(target.resolve()) or not binary.is_file():
                raise RuntimeError(f"Invalid {feature} artifact: {binary}")
            binaries[name] = binary
    if set(binaries) != {"gwi", "cli_test"}:
        raise RuntimeError(f"Missing {feature} Cargo executables: {binaries}")
    listed = probe.start(f"list-{feature}", [str(binaries["cli_test"]), TEST, "--exact", "--list"],
                         "listing", 30)
    require_pass(listed, probe.wait(listed))
    if (listed.directory / "stdout.log").read_text().splitlines().count(f"{TEST}: test") != 1:
        raise RuntimeError(f"Missing exact test: {TEST}")
    return binaries


def phases(output):
    """Keep the raw event text, including reader timestamps and errors."""
    return [line for line in output.splitlines() if any(line.startswith(p + " at ") for p in PHASES)]


def test_passed(output, expected=None):
    summaries = re.findall(r"^test result: ok\. (\d+) passed;", output, re.MULTILINE)
    return bool(summaries and int(summaries[-1]) > 0
                and (expected is None or int(summaries[-1]) == expected))


def measure(probe, feature, binaries, args, summary):
    env = os.environ.copy()
    env["INSTA_WORKSPACE_ROOT"] = str(ROOT)
    workers = []
    serial = 0
    for index in range(args.cpu_workers):
        workers.append(probe.start(f"{feature}-cpu-{index}", [sys.executable, "-c", BURNER],
                                   "cpu", args.max_seconds))
    slots = [None] * args.concurrency
    success = True
    try:
        for repetition in range(1, args.repetitions + 1):
            # Launch workload slots before the first focused invocation.
            for index in range(len(slots)):
                if slots[index] is None:
                    serial += 1
                    command = ([str(binaries["gwi"]), "--help"] if args.workload == "startup" else
                               [str(binaries["cli_test"]), "--skip", TEST, "--nocapture",
                                "--test-threads", str(args.harness_threads), "--color", "never"])
                    slots[index] = probe.start(f"{feature}-workload-{serial}", command,
                                              args.workload, args.workload_timeout, env)
            run = probe.start(f"{feature}-run-{repetition}",
                              [str(binaries["cli_test"]), TEST, "--exact", "--nocapture", "--color", "never"],
                              "focused", args.run_timeout, env)
            while True:
                probe.check_deadline()
                # Record all workload exits, including failures; replenish live slots.
                for index, worker in enumerate(slots):
                    result = worker.poll()
                    if result is not None:
                        if result["outcome"] != "passed" or (args.workload == "harness" and not test_passed(worker.output())):
                            success = False
                        serial += 1
                        slots[index] = probe.start(f"{feature}-workload-{serial}", worker.record["command"],
                                                  args.workload, args.workload_timeout, env)
                for worker in workers:
                    if worker.poll() is not None:
                        success = False
                result = run.poll()
                if result is not None:
                    break
                time.sleep(0.05)
            output = run.output()
            entry = {"feature": feature, "repetition": repetition,
                     "invocation": "first-focused" if repetition == 1 else "subsequent-focused",
                     "directory": run.directory.name, **result, "phases": phases(output)}
            entry["valid_selection"] = test_passed(output, expected=1) and len(entry["phases"]) == 6
            if result["outcome"] == "passed" and not entry["valid_selection"]:
                entry["outcome"] = "invalid-output"
            summary["repetitions"].append(entry)
            write_json(probe.output / "summary.json", summary)
            success &= entry["outcome"] == "passed"
            print(f"{feature} repetition {repetition}: {entry['outcome']} ({result['elapsed_seconds']:.3f}s)", flush=True)
    finally:
        # Capture completed failures before intentionally stopping ongoing activity.
        for worker in workers + [slot for slot in slots if slot is not None]:
            if not worker.done:
                result = worker.poll()
                if result is not None and result["outcome"] != "passed":
                    success = False
                elif result is not None and args.workload == "harness" and worker in slots and not test_passed(worker.output()):
                    success = False
                worker.finish("stopped")
    return success


def positive(value):
    number = float(value)
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError("must be finite and positive")
    return number


def count(value):
    number = int(value)
    if number < 0:
        raise argparse.ArgumentTypeError("must be nonnegative")
    return number


def arguments(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path, help="new directory for builds, logs and metadata")
    parser.add_argument("--features", choices=("both", "default", "mcp"), default="both")
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--cpu-workers", type=count, default=0)
    parser.add_argument("--workload", choices=("none", "startup", "harness"), default="none")
    parser.add_argument("--concurrency", type=count, default=0)
    parser.add_argument("--harness-threads", type=int, default=4)
    for name, default in (("run-timeout", 60), ("workload-timeout", 300), ("build-timeout", 900), ("max-seconds", 2400)):
        parser.add_argument("--" + name, type=positive, default=default)
    args = parser.parse_args(argv)
    if args.repetitions < 1 or args.harness_threads < 1:
        parser.error("repetitions and harness-threads must be positive")
    if (args.workload == "none") != (args.concurrency == 0):
        parser.error("workload and positive concurrency must be specified together")
    return args


def interrupted(signum, frame):
    # Defer interruption until the newly spawned session has been registered.
    # Signals during cleanup cannot interrupt reaping or evidence writes.
    global INTERRUPTED
    INTERRUPTED = signum


def main(argv=None):
    global INTERRUPTED
    INTERRUPTED = None
    args = arguments(argv)
    if sys.platform not in ("linux", "darwin"):
        print("This probe supports Linux and macOS", file=sys.stderr)
        return 2
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    probe = Probe(output, args.max_seconds)
    summary = {"started_utc": datetime.now(timezone.utc).isoformat(), "platform": platform.platform(),
               "cpu_count": os.cpu_count(), "configuration": {**vars(args), "output": str(output)},
               "repetitions": [], "cache_control": "none; first-focused is invocation order only"}
    previous = {sig: signal.signal(sig, interrupted) for sig in (signal.SIGINT, signal.SIGTERM)}
    status = 1
    try:
        revision = probe.start("revision", ["git", "-C", str(ROOT), "rev-parse", "HEAD"], "metadata", 10)
        require_pass(revision, probe.wait(revision))
        summary["revision"] = revision.output().strip()
        dirty = probe.start("status", ["git", "-C", str(ROOT), "status", "--porcelain"], "metadata", 10)
        require_pass(dirty, probe.wait(dirty))
        summary["git_status"] = dirty.output().strip()
        write_json(output / "summary.json", summary)
        features = ("default", "mcp") if args.features == "both" else (args.features,)
        # Finish both isolated builds before any workload begins.
        binaries = {feature: build(probe, feature, args.build_timeout) for feature in features}
        summary["binaries"] = {f: {k: str(v) for k, v in b.items()} for f, b in binaries.items()}
        success = True
        for feature in features:
            success = measure(probe, feature, binaries[feature], args, summary) and success
        probe.check_deadline()
        status = 0 if success else 1
    except (OSError, RuntimeError, ValueError, KeyError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        summary["error"] = str(error)
        status = 130 if isinstance(error, KeyboardInterrupt) else 1
        print(error, file=sys.stderr)
    finally:
        try:
            probe.cleanup()
        except RuntimeError as error:
            summary["cleanup_error"] = str(error)
            status = 1
            print(error, file=sys.stderr)
        summary.update(status=status, commands=[run.record for run in probe.runs])
        write_json(output / "summary.json", summary)
        for sig, handler in previous.items():
            signal.signal(sig, handler)
    print(f"Evidence: {output}", flush=True)
    return status


if __name__ == "__main__":
    sys.exit(main())
