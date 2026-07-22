#!/usr/bin/env python3

"""Run bounded rust-glancer debugging workflows with owned artifacts and cleanup."""

import asyncio
import errno
import json
import os
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
import re
import secrets
import shutil
import signal
import statistics
import subprocess
import sys
from typing import Any, Callable, Dict, List, Optional, Sequence, Set, Tuple


TOOL_FILE = Path(__file__).resolve()
WORKSPACE_ROOT = TOOL_FILE.parent.parent
DEBUG_ROOT = WORKSPACE_ROOT / "target" / "agent-debug"
BUILD_ROOT = DEBUG_ROOT / "build"
RUST_GLANCER_PACKAGE = "rust-glancer"
MODES = {"analyze", "compare-lsp", "fixture", "help", "last", "lsp-query", "test"}
ADMINISTRATIVE_MODES = {"fixture", "last"}
MANAGED_TMPDIR_MODES = {"analyze", "compare-lsp", "lsp-query"}

PLATFORM = "linux" if sys.platform.startswith("linux") else sys.platform
SUPPORTED_PLATFORMS = {"darwin", "linux"}
DEFAULT_TIMEOUTS_MS = {
    "analyze": 15 * 60_000,
    "compare-lsp": 20 * 60_000,
    "lsp-query": 5 * 60_000,
    "test": 15 * 60_000,
}
BUILD_TIMEOUT_MS = 20 * 60_000
MAX_TIMEOUT_MS = 60 * 60_000
TERMINATION_GRACE_SECONDS = 3
MAX_LOG_BYTES = 64 * 1024 * 1024
MAX_REPETITIONS = 20
MAX_WARMUPS = 10
FIXTURE_MARKER = ".rust-glancer-agent-debug-fixture"

ACTIVE_STOP: Optional[Callable[[Dict[str, str]], "asyncio.Task[None]"]] = None
RECEIVED_SIGNAL: Optional[signal.Signals] = None
HOST_TARGET: Optional[str] = None


class CliError(Exception):
    def __init__(self, message: str, exit_code: int = 2) -> None:
        super().__init__(message)
        self.exit_code = exit_code


@dataclass
class RunnerOptions:
    backtrace: Optional[str] = None
    build_enabled: bool = True
    build_profile: str = "release"
    dry_run: bool = False
    environment: List[Tuple[str, str]] = field(default_factory=list)
    isolated_cache: Optional[str] = None
    log_filter: Optional[str] = None
    measure: bool = False
    repeat: int = 1
    rust_analyzer: Optional[str] = None
    sample_on_timeout: bool = False
    timeout_ms: Optional[int] = None
    warmup: int = 0


@dataclass
class ParsedCli:
    help: bool
    options: RunnerOptions
    mode: Optional[str]
    mode_args: List[str]


@dataclass
class CommandSpec:
    command: str
    args: List[str]


def usage() -> str:
    return f"""
Usage:
  just agent-debug [runner-options] analyze <path> [analyze-options...]
  just agent-debug [runner-options] compare-lsp [fixture] [compare-options...]
  just agent-debug [runner-options] lsp-query [query-options...]
  just agent-debug [runner-options] test [nextest-options...]
  just agent-debug fixture <init|list|path|reset> [name]
  just agent-debug last [--json]

Runner options (must appear before the mode):
  --timeout <duration>          Per-run timeout with a required suffix: 90s, 5m, or 1h
  --measure                     Record OS time and peak RSS for runtime runs (not the build)
  --log <filter>                Set RUST_GLANCER_LOG for the debugged process
  --backtrace                   Set RUST_BACKTRACE=1
  --full-backtrace              Set RUST_BACKTRACE=full
  --env <name=value>            Repeatable runtime environment override
  --build-profile <profile>     release (default) or debug
  --no-build                    Intentionally use the existing managed binary
  --sample-on-timeout           On macOS, sample owned rust-glancer processes before cleanup
  --repeat <count>              Number of measured/debug runs (default 1, max {MAX_REPETITIONS})
  --warmup <count>              Unmeasured warm-up runs (default 0, max {MAX_WARMUPS})
  --isolated-cache [name]       Put CARGO_TARGET_DIR under target/agent-debug/cache
  --rust-analyzer <path>        Select the compare-lsp rust-analyzer executable
  --dry-run                     Print the exact argv/environment plan without executing it
  -h, --help                    Show this help

Examples:
  just agent-debug --measure analyze ~/workspace/rust-glancer/reference/rust-analyzer --profile -m
  just agent-debug --log 'rg_lsp_engine=debug' lsp-query --file crates/example.rs hover --marker 'let value'
  just agent-debug --env RUST_GLANCER_PURGE_MEMORY_AFTER_BUILD=0 analyze . --profile
  just agent-debug --timeout 60s --sample-on-timeout test -p rg_analysis inference_test

Everything after the mode is forwarded as an argv array. Shell metacharacters are never evaluated.
Each build has a separate fixed 20m timeout. The runner supports macOS and Linux.
Test mode inherits the system temporary directory so Cargo fixtures stay outside this workspace.
Use --env TMPDIR=<path> before the mode when a test intentionally needs an override.
fixture and last reject runner options, including --dry-run.
Run just agent-debug lsp-query --help for the bounded LSP query syntax.
""".strip()


def next_value(argv: Sequence[str], index: int, option: str) -> str:
    if index + 1 >= len(argv):
        raise CliError("missing value for {}".format(option))
    return argv[index + 1]


def parse_duration(value: str) -> int:
    match = re.fullmatch(r"(\d+)(ms|s|m|h)", value)
    if match is None:
        raise CliError("invalid duration {!r}".format(value))

    amount = int(match.group(1))
    multiplier = {"ms": 1, "s": 1_000, "m": 60_000, "h": 3_600_000}[match.group(2)]
    duration_ms = amount * multiplier
    if duration_ms <= 0 or duration_ms > MAX_TIMEOUT_MS:
        raise CliError("duration must be between 1ms and {}ms".format(MAX_TIMEOUT_MS))
    return duration_ms


def parse_count(value: str, option: str, maximum: int, allow_zero: bool = False) -> int:
    try:
        count = int(value)
    except ValueError as error:
        raise CliError("{} must be an integer".format(option)) from error
    minimum = 0 if allow_zero else 1
    if count < minimum or count > maximum:
        raise CliError("{} must be between {} and {}".format(option, minimum, maximum))
    return count


def parse_environment_assignment(value: str, option: str = "--env") -> Tuple[str, str]:
    separator = value.find("=")
    if separator <= 0:
        raise CliError("{} expects name=value".format(option))

    name = value[:separator]
    assigned_value = value[separator + 1 :]
    if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name) is None:
        raise CliError("{} contains an invalid environment name: {}".format(option, name))
    if "\0" in assigned_value:
        raise CliError("{} value may not contain NUL".format(option))
    return name, assigned_value


def validate_storage_name(value: str, label: str) -> str:
    if re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}", value) is None:
        raise CliError(
            "{} must contain only letters, digits, _ or -, and be at most 64 characters".format(label)
        )
    return value


def parse_cli(argv: Sequence[str]) -> ParsedCli:
    options = RunnerOptions()
    runner_option_seen = False
    mode: Optional[str] = None
    mode_args: List[str] = []

    index = 0
    while index < len(argv):
        argument = argv[index]
        if argument in MODES:
            mode = argument
            mode_args = list(argv[index + 1 :])
            break
        if argument == "--":
            if index + 1 >= len(argv) or argv[index + 1] not in MODES:
                raise CliError("expected a mode after --")
            mode = argv[index + 1]
            mode_args = list(argv[index + 2 :])
            break

        runner_option_seen = True
        if argument in {"--help", "-h"}:
            return ParsedCli(True, options, None, [])
        if argument == "--timeout":
            options.timeout_ms = parse_duration(next_value(argv, index, argument))
            index += 2
            continue
        if argument == "--measure":
            options.measure = True
        elif argument == "--log":
            options.log_filter = next_value(argv, index, argument)
            index += 1
        elif argument == "--backtrace":
            options.backtrace = "1"
        elif argument == "--full-backtrace":
            options.backtrace = "full"
        elif argument == "--env":
            options.environment.append(
                parse_environment_assignment(next_value(argv, index, argument), argument)
            )
            index += 1
        elif argument == "--build-profile":
            options.build_profile = next_value(argv, index, argument)
            if options.build_profile not in {"debug", "release"}:
                raise CliError("--build-profile must be debug or release")
            index += 1
        elif argument == "--no-build":
            options.build_enabled = False
        elif argument == "--sample-on-timeout":
            options.sample_on_timeout = True
        elif argument == "--repeat":
            options.repeat = parse_count(
                next_value(argv, index, argument), argument, MAX_REPETITIONS
            )
            index += 1
        elif argument == "--warmup":
            options.warmup = parse_count(
                next_value(argv, index, argument), argument, MAX_WARMUPS, allow_zero=True
            )
            index += 1
        elif argument == "--isolated-cache":
            candidate = argv[index + 1] if index + 1 < len(argv) else None
            if candidate is None or candidate.startswith("-") or candidate in MODES:
                cache_name = "default"
            else:
                cache_name = candidate
                index += 1
            options.isolated_cache = validate_storage_name(cache_name, "isolated cache name")
        elif argument == "--rust-analyzer":
            options.rust_analyzer = next_value(argv, index, argument)
            index += 1
        elif argument == "--dry-run":
            options.dry_run = True
        else:
            raise CliError(
                "unknown runner option {}; runner options must precede the mode".format(argument)
            )
        index += 1

    if mode is None:
        raise CliError("missing mode; run just agent-debug --help")
    if mode in ADMINISTRATIVE_MODES and runner_option_seen:
        raise CliError("{} does not accept runner options".format(mode))
    return ParsedCli(False, options, mode, mode_args)


def normalized_mode_arguments(mode: str, mode_args: Sequence[str]) -> List[str]:
    arguments = list(mode_args)
    if mode == "compare-lsp" and (not arguments or arguments[0].startswith("-")):
        return ["rust_analyzer"] + arguments
    if mode == "lsp-query":
        for index, argument in enumerate(arguments):
            if argument == "--query-file" and index + 1 < len(arguments) and arguments[index + 1] == "-":
                raise CliError("--query-file - is unsupported; use --query-json or a plan file")
        if arguments and not arguments[0].startswith("-") and arguments[0].endswith(".json"):
            return ["--query-file"] + arguments
    return arguments


def host_target() -> str:
    global HOST_TARGET
    if HOST_TARGET is not None:
        return HOST_TARGET

    try:
        rustc = subprocess.run(
            ["rustc", "-vV"],
            cwd=str(WORKSPACE_ROOT),
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=10,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise CliError("failed to determine the host Rust target: {}".format(error)) from error
    if rustc.returncode != 0:
        reason = rustc.stderr.strip() or "rustc exited with {}".format(rustc.returncode)
        raise CliError("failed to determine the host Rust target: {}".format(reason))

    match = re.search(r"^host:\s*(\S+)\s*$", rustc.stdout, re.MULTILINE)
    if match is None or re.fullmatch(r"[A-Za-z0-9_.-]+", match.group(1)) is None:
        raise CliError("rustc -vV did not report a valid host target")
    HOST_TARGET = match.group(1)
    return HOST_TARGET


def rust_glancer_binary(profile: str) -> Path:
    return BUILD_ROOT / host_target() / profile / "rust-glancer"


def build_spec(options: RunnerOptions) -> CommandSpec:
    # Pin every Cargo setting that controls the executable location. Inherited Cargo
    # configuration cannot leave the runner executing an older binary from another target.
    args = ["build", "--target-dir", str(BUILD_ROOT), "--target", host_target()]
    if options.build_profile == "release":
        args.append("--release")
    args.extend(["-p", RUST_GLANCER_PACKAGE])
    return CommandSpec("cargo", args)


def runtime_spec(mode: str, mode_args: Sequence[str], options: RunnerOptions) -> CommandSpec:
    arguments = normalized_mode_arguments(mode, mode_args)
    if mode in {"analyze", "compare-lsp"}:
        return CommandSpec(
            str(rust_glancer_binary(options.build_profile)), [mode] + arguments
        )
    if mode == "lsp-query":
        return CommandSpec(
            sys.executable,
            [
                str(WORKSPACE_ROOT / "tools" / "lsp-query.py"),
                "--binary",
                str(rust_glancer_binary(options.build_profile)),
            ]
            + arguments,
        )
    if mode == "test":
        return CommandSpec("cargo", ["nextest", "run"] + arguments)
    raise CliError("mode {} does not run a managed process".format(mode))


def is_lsp_query_help(mode: str, mode_args: Sequence[str]) -> bool:
    return mode == "lsp-query" and len(mode_args) == 1 and mode_args[0] in {"-h", "--help"}


def should_build(mode: str, mode_args: Sequence[str], options: RunnerOptions) -> bool:
    return options.build_enabled and mode != "test" and not is_lsp_query_help(mode, mode_args)


def runtime_environment(
    mode: str, options: RunnerOptions, run_directory: Path
) -> Dict[str, str]:
    environment = dict(os.environ)

    # Cargo-backed tests create standalone projects through `tempfile`. Putting those projects
    # below rust-glancer changes Cargo workspace discovery, so only runtime debugging modes use
    # the runner-owned scratch directory by default.
    if mode in MANAGED_TMPDIR_MODES:
        environment["TMPDIR"] = str(run_directory / "tmp")
    environment["RUST_GLANCER_AGENT_DEBUG"] = "1"
    environment.update(options.environment)

    if options.log_filter is not None:
        environment["RUST_GLANCER_LOG"] = options.log_filter
    if options.backtrace is not None:
        environment["RUST_BACKTRACE"] = options.backtrace
    if options.rust_analyzer is not None:
        environment["RUST_GLANCER_COMPARE_LSP_RUST_ANALYZER"] = options.rust_analyzer
    if options.isolated_cache is not None:
        cache = DEBUG_ROOT / "cache" / options.isolated_cache
        cache.mkdir(parents=True, exist_ok=True)
        environment["CARGO_TARGET_DIR"] = str(cache)
    return environment


def build_environment(run_directory: Path) -> Dict[str, str]:
    environment = dict(os.environ)
    environment["TMPDIR"] = str(run_directory / "tmp")
    return environment


def is_sensitive_environment_name(name: str) -> bool:
    return re.search(r"TOKEN|SECRET|PASSWORD|PASSWD|API_KEY|PRIVATE_KEY|CREDENTIAL", name, re.I) is not None


def displayed_assignments(assignments: Sequence[Tuple[str, str]]) -> Dict[str, str]:
    return {
        name: "<redacted>" if is_sensitive_environment_name(name) else value
        for name, value in assignments
    }


def utc_timestamp() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def create_run_directory(mode: str) -> Path:
    DEBUG_ROOT.mkdir(parents=True, exist_ok=True)
    timestamp = re.sub(r"[-:.]", "", utc_timestamp())
    suffix = secrets.token_hex(3)
    run_directory = DEBUG_ROOT / "runs" / "{}-{}-{}-{}".format(
        timestamp, mode, os.getpid(), suffix
    )
    (run_directory / "tmp").mkdir(parents=True)
    return run_directory


def without_none(value: Any) -> Any:
    if isinstance(value, dict):
        return {key: without_none(item) for key, item in value.items() if item is not None}
    if isinstance(value, list):
        return [without_none(item) for item in value]
    return value


def write_json(file_path: Path, value: Any) -> None:
    file_path.write_text(
        json.dumps(without_none(value), indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )


class BoundedLog:
    def __init__(self, file_path: Path, output: Any, label: str) -> None:
        self.file = file_path.open("wb")
        self.output = output
        self.label = label
        self.written = 0
        self.truncated = False

    def write(self, chunk: bytes) -> None:
        remaining = max(0, MAX_LOG_BYTES - self.written)
        if remaining > 0:
            retained = chunk[:remaining]
            self.file.write(retained)
            self.output.write(retained)
            self.output.flush()
            self.written += len(retained)
        if len(chunk) > remaining and not self.truncated:
            notice = "\n<agent-debug {} truncated after {} bytes>\n".format(
                self.label, MAX_LOG_BYTES
            ).encode("utf-8")
            self.file.write(notice)
            self.output.write(notice)
            self.output.flush()
            self.truncated = True

    def close(self) -> None:
        self.file.close()


def group_exists(process_group_id: int) -> bool:
    try:
        os.killpg(process_group_id, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def signal_process_group(process_id: Optional[int], sig: signal.Signals) -> None:
    if process_id is None:
        return
    try:
        os.killpg(process_id, sig)
    except OSError as error:
        if error.errno not in {errno.EPERM, errno.ESRCH}:
            raise


def process_group_snapshot(process_group_id: int) -> Dict[str, Any]:
    try:
        completed = subprocess.run(
            ["/bin/ps", "-axo", "pid=,ppid=,pgid=,state=,etime=,command="],
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=2,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"text": "process snapshot failed: {}\n".format(error), "processes": []}
    if completed.returncode != 0:
        reason = completed.stderr.strip() or "ps exited with {}".format(completed.returncode)
        return {"text": "process snapshot failed: {}\n".format(reason), "processes": []}

    processes = []
    for line in completed.stdout.splitlines():
        columns = line.strip().split(maxsplit=5)
        if len(columns) != 6:
            continue
        try:
            pid, ppid, pgid = (int(columns[index]) for index in range(3))
        except ValueError:
            continue
        if pgid != process_group_id:
            continue
        processes.append(
            {
                "pid": pid,
                "ppid": ppid,
                "pgid": pgid,
                "state": columns[3],
                "elapsed": columns[4],
                "command": columns[5],
            }
        )

    lines = ["PID PPID PGID STATE ELAPSED COMMAND"]
    lines.extend(
        "{pid} {ppid} {pgid} {state} {elapsed} {command}".format(**entry)
        for entry in processes
    )
    return {"text": "\n".join(lines) + "\n", "processes": processes}


def snapshot_inspection_failed(snapshot: Dict[str, Any]) -> bool:
    return snapshot["text"].startswith("process snapshot failed:")


async def run_auxiliary(command: str, args: Sequence[str], timeout_seconds: float) -> Dict[str, Any]:
    try:
        child = await asyncio.create_subprocess_exec(
            command,
            *args,
            stdin=asyncio.subprocess.DEVNULL,
            stdout=asyncio.subprocess.DEVNULL,
            stderr=asyncio.subprocess.PIPE,
        )
    except OSError as error:
        return {"ok": False, "message": str(error)}

    try:
        _, stderr = await asyncio.wait_for(child.communicate(), timeout_seconds)
    except asyncio.TimeoutError:
        child.kill()
        _, stderr = await child.communicate()
    message = stderr.decode("utf-8", errors="replace").strip()
    return {"ok": child.returncode == 0, "message": message, "code": child.returncode}


async def sample_owned_processes(snapshot: Dict[str, Any], output_directory: Path) -> List[Dict[str, Any]]:
    sample = Path("/usr/bin/sample")
    if PLATFORM != "darwin" or not sample.exists():
        return []

    engines = [entry for entry in snapshot["processes"] if "lsp-engine" in entry["command"]]
    rust_glancer = [
        entry
        for entry in snapshot["processes"]
        if "rust-glancer" in entry["command"] and "lsp-engine" not in entry["command"]
    ]
    targets = (engines if engines else rust_glancer)[:2]
    results = []
    for entry in targets:
        output = output_directory / "sample-{}.txt".format(entry["pid"])
        result = await run_auxiliary(
            str(sample),
            [str(entry["pid"]), "1", "5", "-mayDie", "-file", str(output)],
            5,
        )
        results.append({"pid": entry["pid"], "output": str(output), **result})
    return results


async def clean_owned_process_group(
    process_id: Optional[int], output_directory: Path
) -> Dict[str, Any]:
    if process_id is None:
        cleanup = {"status": "not-spawned", "processGroupId": None, "verifiedEmpty": None}
        write_json(output_directory / "cleanup.json", cleanup)
        return cleanup

    # Only inspect and signal the session created for this command. This keeps cleanup independent
    # from every other rust-glancer process running on the developer's machine.
    initial = process_group_snapshot(process_id)
    initial_failed = snapshot_inspection_failed(initial)
    initial_pids = [entry["pid"] for entry in initial["processes"]]
    term_sent = False
    kill_sent = False

    if initial_pids:
        (output_directory / "cleanup-processes.txt").write_text(initial["text"], encoding="utf-8")
    if initial_pids or (initial_failed and group_exists(process_id)):
        term_sent = True
        signal_process_group(process_id, signal.SIGTERM)
        await asyncio.sleep(0.1)
        after_term = process_group_snapshot(process_id)
        if after_term["processes"] or (
            snapshot_inspection_failed(after_term) and group_exists(process_id)
        ):
            kill_sent = True
            signal_process_group(process_id, signal.SIGKILL)

    final = initial
    for _ in range(10):
        final = process_group_snapshot(process_id)
        if not snapshot_inspection_failed(final) and not final["processes"]:
            break
        if snapshot_inspection_failed(final) and not group_exists(process_id):
            break
        await asyncio.sleep(0.05)

    final_failed = snapshot_inspection_failed(final)
    remaining_pids = [entry["pid"] for entry in final["processes"]]
    verified_empty = not group_exists(process_id) if final_failed else not remaining_pids
    if remaining_pids:
        (output_directory / "cleanup-processes-remaining.txt").write_text(
            final["text"], encoding="utf-8"
        )
    status = "unverified"
    if verified_empty:
        status = "survivors-terminated" if initial_pids or term_sent else "verified-empty"
    cleanup = {
        "status": status,
        "processGroupId": process_id,
        "initialPids": initial_pids,
        "termSent": term_sent,
        "killSent": kill_sent,
        "verifiedEmpty": verified_empty,
        "remainingPids": remaining_pids,
        "inspectionError": final["text"].strip() if final_failed else None,
    }
    write_json(output_directory / "cleanup.json", cleanup)
    return cleanup


def measurement_command(command: str, args: Sequence[str], output_file: Path) -> CommandSpec:
    time_command = Path("/usr/bin/time")
    if PLATFORM == "darwin" and time_command.exists():
        return CommandSpec(
            str(time_command), ["-l", "-p", "-o", str(output_file), command] + list(args)
        )
    if PLATFORM == "linux" and time_command.exists():
        return CommandSpec(
            str(time_command), ["-v", "-o", str(output_file), command] + list(args)
        )
    raise CliError("--measure requires /usr/bin/time on macOS or Linux")


def parse_time_report(raw: str, platform: str = PLATFORM) -> Dict[str, Any]:
    metrics: Dict[str, Any] = {"platform": platform, "rawAvailable": bool(raw)}
    if platform == "darwin":
        for line in raw.splitlines():
            time_match = re.fullmatch(r"\s*(real|user|sys)\s+([0-9.]+)\s*", line)
            if time_match is not None:
                metrics[time_match.group(1) + "Seconds"] = float(time_match.group(2))
                continue
            rss_match = re.fullmatch(r"\s*(\d+)\s+maximum resident set size\s*", line)
            if rss_match is not None:
                metrics["peakRssBytes"] = int(rss_match.group(1))
            footprint_match = re.fullmatch(r"\s*(\d+)\s+peak memory footprint\s*", line)
            if footprint_match is not None:
                metrics["peakMemoryFootprintBytes"] = int(footprint_match.group(1))
    elif platform == "linux":
        for line in raw.splitlines():
            rss_match = re.fullmatch(
                r"\s*Maximum resident set size \(kbytes\):\s*(\d+)\s*", line
            )
            if rss_match is not None:
                metrics["peakRssBytes"] = int(rss_match.group(1)) * 1024
            user_match = re.fullmatch(r"\s*User time \(seconds\):\s*([0-9.]+)\s*", line)
            if user_match is not None:
                metrics["userSeconds"] = float(user_match.group(1))
            sys_match = re.fullmatch(r"\s*System time \(seconds\):\s*([0-9.]+)\s*", line)
            if sys_match is not None:
                metrics["sysSeconds"] = float(sys_match.group(1))
    if "peakRssBytes" in metrics:
        metrics["peakRssMiB"] = metrics["peakRssBytes"] / 1024 / 1024
    return metrics


async def pump_stream(reader: "asyncio.StreamReader", log: BoundedLog) -> None:
    while True:
        chunk = await reader.read(64 * 1024)
        if not chunk:
            return
        log.write(chunk)


async def run_supervised(
    spec: CommandSpec,
    cwd: Path,
    environment: Dict[str, str],
    output_directory: Path,
    timeout_ms: int,
    measure: bool = False,
    sample_on_timeout: bool = False,
) -> Dict[str, Any]:
    global ACTIVE_STOP

    output_directory.mkdir(parents=True, exist_ok=True)
    stdout = BoundedLog(output_directory / "stdout.log", sys.stdout.buffer, "stdout")
    stderr = BoundedLog(output_directory / "stderr.log", sys.stderr.buffer, "stderr")
    time_output = output_directory / "time.txt"
    launched = measurement_command(spec.command, spec.args, time_output) if measure else spec
    started_at = datetime.now(timezone.utc)
    started_monotonic = asyncio.get_running_loop().time()

    child_environment = dict(environment)
    child_environment["RUST_GLANCER_AGENT_DEBUG_OUTPUT_DIR"] = str(output_directory)
    try:
        child = await asyncio.create_subprocess_exec(
            launched.command,
            *launched.args,
            cwd=str(cwd),
            env=child_environment,
            stdin=asyncio.subprocess.DEVNULL,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            start_new_session=True,
        )
    except OSError as error:
        stdout.close()
        stderr.close()
        cleanup = await clean_owned_process_group(None, output_directory)
        return {
            "command": spec.command,
            "args": spec.args,
            "pid": None,
            "code": None,
            "signal": None,
            "spawnError": str(error),
            "elapsedMs": int((asyncio.get_running_loop().time() - started_monotonic) * 1000),
            "timedOut": False,
            "cleanup": cleanup,
        }

    write_json(
        output_directory / "process.json",
        {
            "command": spec.command,
            "args": spec.args,
            "launchedCommand": launched.command,
            "launchedArgs": launched.args,
            "pid": child.pid,
            "processGroupId": child.pid,
            "startedAt": started_at.isoformat(timespec="milliseconds").replace("+00:00", "Z"),
        },
    )

    assert child.stdout is not None
    assert child.stderr is not None
    stdout_task = asyncio.create_task(pump_stream(child.stdout, stdout))
    stderr_task = asyncio.create_task(pump_stream(child.stderr, stderr))
    completion = asyncio.create_task(child.wait())
    stop_reason: Optional[Dict[str, str]] = None
    stop_task: Optional["asyncio.Task[None]"] = None

    async def stop_process(reason: Dict[str, str]) -> None:
        if reason["kind"] == "timeout":
            snapshot = process_group_snapshot(child.pid)
            (output_directory / "processes.txt").write_text(snapshot["text"], encoding="utf-8")
            if sample_on_timeout:
                samples = await sample_owned_processes(snapshot, output_directory)
                write_json(output_directory / "samples.json", samples)

        if child.returncode is None:
            signal_process_group(child.pid, signal.SIGTERM)
        try:
            await asyncio.wait_for(asyncio.shield(completion), TERMINATION_GRACE_SECONDS)
        except asyncio.TimeoutError:
            if child.returncode is None:
                signal_process_group(child.pid, signal.SIGKILL)

    def request_stop(reason: Dict[str, str]) -> "asyncio.Task[None]":
        nonlocal stop_reason, stop_task
        if stop_reason is None:
            stop_reason = reason
        if stop_task is None:
            stop_task = asyncio.create_task(stop_process(reason))
        return stop_task

    ACTIVE_STOP = request_stop
    try:
        done, _ = await asyncio.wait({completion}, timeout=timeout_ms / 1000)
        if completion not in done:
            await request_stop({"kind": "timeout"})
        return_code = await completion
        if stop_task is not None:
            await stop_task
    finally:
        ACTIVE_STOP = None

    # A direct child can exit while one of its descendants still owns the output pipes. Clean the
    # session before waiting for EOF so that such a survivor cannot hang the runner indefinitely.
    cleanup = await clean_owned_process_group(child.pid, output_directory)
    await asyncio.gather(stdout_task, stderr_task)
    stdout.close()
    stderr.close()

    elapsed_ms = int((asyncio.get_running_loop().time() - started_monotonic) * 1000)
    metrics = None
    if measure:
        raw = time_output.read_text(encoding="utf-8") if time_output.exists() else ""
        metrics = parse_time_report(raw)
        write_json(output_directory / "metrics.json", metrics)

    signal_name = None
    code: Optional[int] = return_code
    if return_code < 0:
        code = None
        try:
            signal_name = signal.Signals(-return_code).name
        except ValueError:
            signal_name = str(-return_code)
    interrupted_by = stop_reason.get("signal") if stop_reason and stop_reason["kind"] == "signal" else None
    return {
        "command": spec.command,
        "args": spec.args,
        "pid": child.pid,
        "code": code,
        "signal": signal_name,
        "elapsedMs": elapsed_ms,
        "timedOut": bool(stop_reason and stop_reason["kind"] == "timeout"),
        "interruptedBy": interrupted_by,
        "metrics": metrics,
        "cleanup": cleanup,
    }


def exit_code_for(result: Dict[str, Any]) -> int:
    if result.get("spawnError"):
        return 1
    if result.get("timedOut"):
        return 124
    interrupted = result.get("interruptedBy")
    if interrupted == "SIGINT":
        return 130
    if interrupted == "SIGTERM":
        return 143
    if interrupted == "SIGHUP":
        return 129
    if result.get("signal"):
        return 128
    code = result.get("code")
    return code if isinstance(code, int) else 1


def aggregate_results(results: Sequence[Dict[str, Any]]) -> Dict[str, Any]:
    elapsed = [result["elapsedMs"] for result in results]
    peak_rss = [
        result["metrics"]["peakRssBytes"]
        for result in results
        if result.get("metrics") and "peakRssBytes" in result["metrics"]
    ]
    aggregate: Dict[str, Any] = {
        "runs": len(results),
        "elapsedMs": {
            "min": min(elapsed),
            "median": statistics.median(elapsed),
            "max": max(elapsed),
        },
    }
    if peak_rss:
        aggregate["peakRssBytes"] = {
            "min": min(peak_rss),
            "median": statistics.median(peak_rss),
            "max": max(peak_rss),
        }
        aggregate["peakRssMiB"] = {
            key: value / 1024 / 1024 for key, value in aggregate["peakRssBytes"].items()
        }
    return aggregate


def summarize_cleanup(results: Sequence[Dict[str, Any]]) -> Dict[str, Any]:
    cleanups = [result.get("cleanup") for result in results if result.get("cleanup")]
    if not cleanups:
        return {"status": "not-run", "runs": 0}
    verified_runs = sum(cleanup.get("verifiedEmpty") is True for cleanup in cleanups)
    return {
        "status": "verified" if verified_runs == len(cleanups) else "unverified",
        "runs": len(cleanups),
        "verifiedRuns": verified_runs,
    }


def print_run_result(
    run_directory: Path, aggregate: Dict[str, Any], exit_code: int, process_cleanup: Dict[str, Any]
) -> None:
    print("\nagent-debug: run artifacts: {}".format(run_directory), file=sys.stderr)
    if aggregate.get("peakRssMiB"):
        rss = aggregate["peakRssMiB"]
        print(
            "agent-debug: peak RSS MiB min/median/max: {:.1f} / {:.1f} / {:.1f}".format(
                rss["min"], rss["median"], rss["max"]
            ),
            file=sys.stderr,
        )
    print("agent-debug: process cleanup: {}".format(process_cleanup["status"]), file=sys.stderr)
    print("agent-debug: exit code: {}".format(exit_code), file=sys.stderr)


def planned_workflow(mode: str, mode_args: Sequence[str], options: RunnerOptions) -> Dict[str, Any]:
    runtime = runtime_spec(mode, mode_args, options)
    build = build_spec(options) if should_build(mode, mode_args, options) else None
    return {
        "build": None if build is None else {"command": build.command, "args": build.args},
        "runtime": {"command": runtime.command, "args": runtime.args},
        "runner": {
            "timeoutMs": options.timeout_ms or DEFAULT_TIMEOUTS_MS[mode],
            "measure": options.measure,
            "repeat": options.repeat,
            "warmup": options.warmup,
            "sampleOnTimeout": options.sample_on_timeout,
            "buildProfile": options.build_profile,
            "isolatedCache": options.isolated_cache,
            "environment": displayed_assignments(options.environment),
            "logFilter": options.log_filter,
            "backtrace": options.backtrace,
            "rustAnalyzer": options.rust_analyzer,
        },
    }


async def run_managed_workflow(
    mode: str, mode_args: Sequence[str], options: RunnerOptions
) -> int:
    # Validate and normalize mode-specific arguments before creating any run artifacts.
    normalized_arguments = normalized_mode_arguments(mode, mode_args)
    if options.dry_run:
        print(json.dumps(without_none(planned_workflow(mode, mode_args, options)), indent=2))
        return 0

    run_directory = create_run_directory(mode)
    timeout_ms = options.timeout_ms or DEFAULT_TIMEOUTS_MS[mode]
    workflow = planned_workflow(mode, mode_args, options)
    write_json(
        run_directory / "metadata.json",
        {
            "schema": "rust-glancer-agent-debug/v1",
            "mode": mode,
            "modeArgs": normalized_arguments,
            "startedAt": utc_timestamp(),
            "cwd": str(WORKSPACE_ROOT),
            "runner": workflow["runner"],
        },
    )

    build_result = None
    if should_build(mode, mode_args, options):
        build_result = await run_supervised(
            build_spec(options),
            WORKSPACE_ROOT,
            build_environment(run_directory),
            run_directory / "build",
            BUILD_TIMEOUT_MS,
        )
        if exit_code_for(build_result) != 0:
            process_cleanup = summarize_cleanup([build_result])
            summary = {
                "status": "build-failed",
                "exitCode": exit_code_for(build_result),
                "build": build_result,
                "processCleanup": process_cleanup,
            }
            write_json(run_directory / "summary.json", summary)
            print_run_result(run_directory, {}, summary["exitCode"], process_cleanup)
            return summary["exitCode"]

    runtime = runtime_spec(mode, mode_args, options)
    if mode in {"analyze", "compare-lsp"} and not rust_glancer_binary(
        options.build_profile
    ).exists():
        raise CliError(
            "{} does not exist; remove --no-build or build it first".format(
                rust_glancer_binary(options.build_profile)
            )
        )

    environment = runtime_environment(mode, options, run_directory)
    warmups = []
    for index in range(options.warmup):
        result = await run_supervised(
            runtime,
            WORKSPACE_ROOT,
            environment,
            run_directory / "warmup-{:03d}".format(index + 1),
            timeout_ms,
            sample_on_timeout=options.sample_on_timeout,
        )
        warmups.append(result)
        if exit_code_for(result) != 0 or RECEIVED_SIGNAL is not None:
            break

    results = []
    if all(exit_code_for(result) == 0 for result in warmups) and RECEIVED_SIGNAL is None:
        for index in range(options.repeat):
            result = await run_supervised(
                runtime,
                WORKSPACE_ROOT,
                environment,
                run_directory / "run-{:03d}".format(index + 1),
                timeout_ms,
                measure=options.measure,
                sample_on_timeout=options.sample_on_timeout,
            )
            results.append(result)
            if exit_code_for(result) != 0 or RECEIVED_SIGNAL is not None:
                break

    terminal_result = results[-1] if results else warmups[-1] if warmups else None
    exit_code = exit_code_for(terminal_result) if terminal_result is not None else 1
    aggregate = aggregate_results(results) if results else {"runs": 0}
    cleanup_inputs = [result for result in [build_result] + warmups + results if result is not None]
    process_cleanup = summarize_cleanup(cleanup_inputs)
    status = "completed" if exit_code == 0 else "failed"
    if terminal_result is not None and terminal_result.get("timedOut"):
        status = "timed-out"
    summary = {
        "status": status,
        "exitCode": exit_code,
        "completedAt": utc_timestamp(),
        "build": build_result,
        "warmups": warmups,
        "results": results,
        "aggregate": aggregate,
        "processCleanup": process_cleanup,
    }
    write_json(run_directory / "summary.json", summary)
    print_run_result(run_directory, aggregate, exit_code, process_cleanup)
    return exit_code


def fixture_directory(name: str) -> Path:
    return DEBUG_ROOT / "fixtures" / validate_storage_name(name, "fixture name")


def fixture_has_marker(directory: Path) -> bool:
    try:
        return (
            directory.is_dir()
            and not directory.is_symlink()
            and (directory / FIXTURE_MARKER).is_file()
            and not (directory / FIXTURE_MARKER).is_symlink()
        )
    except OSError:
        return False


def assert_safe_fixture_entry(entry: Path, kind: str, label: str) -> None:
    try:
        entry.lstat()
    except FileNotFoundError:
        return
    matches_kind = entry.is_dir() if kind == "directory" else entry.is_file()
    if not matches_kind or entry.is_symlink():
        raise CliError("{} must be a real {}: {}".format(label, kind, entry))


def run_fixture(mode_args: Sequence[str]) -> int:
    if not mode_args or len(mode_args) > 2:
        raise CliError("fixture expects <init|list|path|reset> [name]")
    operation = mode_args[0]
    name = mode_args[1] if len(mode_args) == 2 else None

    fixtures_root = DEBUG_ROOT / "fixtures"
    fixtures_root.mkdir(parents=True, exist_ok=True)
    if not fixtures_root.is_dir() or fixtures_root.is_symlink():
        raise CliError("fixture root must be a real directory: {}".format(fixtures_root))
    if operation == "list":
        if name is not None:
            raise CliError("fixture list does not accept a name")
        for entry in sorted(fixtures_root.iterdir(), key=lambda item: item.name):
            if fixture_has_marker(entry):
                print(entry.name)
        return 0
    if name is None:
        raise CliError("fixture {} requires a name".format(operation))

    directory = fixture_directory(name)
    if operation == "path":
        if not fixture_has_marker(directory):
            raise CliError("fixture does not exist: {}".format(name))
        print(directory)
        return 0
    if operation == "init":
        if directory.exists() or directory.is_symlink():
            if not directory.is_dir() or directory.is_symlink():
                raise CliError(
                    "refusing to initialize through a non-directory fixture path: {}".format(name)
                )
            if not fixture_has_marker(directory) and any(directory.iterdir()):
                raise CliError("refusing to adopt a non-empty unmanaged fixture: {}".format(name))
        directory.mkdir(parents=True, exist_ok=True)
        source_directory = directory / "src"
        manifest = directory / "Cargo.toml"
        source = source_directory / "lib.rs"
        marker = directory / FIXTURE_MARKER
        assert_safe_fixture_entry(source_directory, "directory", "fixture source directory")
        source_directory.mkdir(parents=True, exist_ok=True)
        assert_safe_fixture_entry(manifest, "file", "fixture manifest")
        assert_safe_fixture_entry(source, "file", "fixture source")
        assert_safe_fixture_entry(marker, "file", "fixture marker")
        if not manifest.exists():
            package_name = name.replace("-", "_")
            manifest.write_text(
                "[package]\nname = {}\nversion = \"0.1.0\"\nedition = \"2024\"\n\n"
                "[workspace]\nresolver = \"3\"\n".format(json.dumps(package_name)),
                encoding="utf-8",
            )
        if not source.exists():
            source.write_text("pub fn reproduce() {}\n", encoding="utf-8")
        marker.write_text("managed by just agent-debug fixture\n", encoding="utf-8")
        print(directory)
        return 0
    if operation == "reset":
        if not fixture_has_marker(directory):
            raise CliError("refusing to reset unmanaged fixture: {}".format(name))
        shutil.rmtree(str(directory))
        return 0
    raise CliError("unknown fixture operation {}".format(operation))


def last_run_directory() -> Path:
    runs = DEBUG_ROOT / "runs"
    if not runs.exists():
        raise CliError("no agent-debug runs exist yet")
    entries = sorted((entry for entry in runs.iterdir() if entry.is_dir()), key=lambda item: item.name)
    if not entries:
        raise CliError("no agent-debug runs exist yet")
    return entries[-1]


def run_last(mode_args: Sequence[str]) -> int:
    if len(mode_args) > 1 or (mode_args and mode_args[0] != "--json"):
        raise CliError("last accepts only --json")
    run_directory = last_run_directory()
    summary_file = run_directory / "summary.json"
    summary = json.loads(summary_file.read_text(encoding="utf-8")) if summary_file.exists() else None
    if mode_args:
        print(json.dumps({"runDirectory": str(run_directory), "summary": summary}, indent=2))
        return 0

    print("run: {}".format(run_directory))
    print("status: {}".format(summary.get("status", "incomplete") if summary else "incomplete"))
    if summary and "exitCode" in summary:
        print("exit code: {}".format(summary["exitCode"]))
    if summary and summary.get("aggregate", {}).get("peakRssMiB"):
        rss = summary["aggregate"]["peakRssMiB"]
        print(
            "peak RSS MiB min/median/max: {:.1f} / {:.1f} / {:.1f}".format(
                rss["min"], rss["median"], rss["max"]
            )
        )
    if summary and summary.get("processCleanup", {}).get("status"):
        print("process cleanup: {}".format(summary["processCleanup"]["status"]))
    return 0


def install_signal_handlers() -> None:
    loop = asyncio.get_running_loop()

    def receive(sig: signal.Signals) -> None:
        global RECEIVED_SIGNAL
        if RECEIVED_SIGNAL is None:
            RECEIVED_SIGNAL = sig
        if ACTIVE_STOP is not None:
            ACTIVE_STOP({"kind": "signal", "signal": sig.name})

    for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        loop.add_signal_handler(sig, receive, sig)


async def async_main(argv: Sequence[str]) -> int:
    if PLATFORM not in SUPPORTED_PLATFORMS:
        raise CliError("unsupported platform {}; agent-debug requires macOS or Linux".format(PLATFORM))
    install_signal_handlers()
    parsed = parse_cli(argv)
    if parsed.help or parsed.mode == "help":
        print(usage())
        return 0
    assert parsed.mode is not None
    if is_lsp_query_help(parsed.mode, parsed.mode_args):
        # Help is a parser operation, not a debugging run. Execute it directly so routine command
        # discovery cannot create artifacts or replace the result reported by `agent-debug last`.
        try:
            help_process = subprocess.run(
                [sys.executable, str(WORKSPACE_ROOT / "tools" / "lsp-query.py")]
                + parsed.mode_args,
                cwd=str(WORKSPACE_ROOT),
                check=False,
                stdin=subprocess.DEVNULL,
                timeout=10,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise CliError("failed to show lsp-query help: {}".format(error), exit_code=1) from error
        return help_process.returncode
    if parsed.mode == "fixture":
        return run_fixture(parsed.mode_args)
    if parsed.mode == "last":
        return run_last(parsed.mode_args)
    return await run_managed_workflow(parsed.mode, parsed.mode_args, parsed.options)


def main() -> int:
    try:
        exit_code = asyncio.run(async_main(sys.argv[1:]))
    except CliError as error:
        print("agent-debug: {}".format(error), file=sys.stderr)
        return error.exit_code
    except KeyboardInterrupt:
        return 130
    except Exception as error:
        print("agent-debug: unexpected failure: {}".format(error), file=sys.stderr)
        return 1
    if RECEIVED_SIGNAL is not None:
        return 128 + RECEIVED_SIGNAL.value
    return exit_code


if __name__ == "__main__":
    sys.exit(main())
