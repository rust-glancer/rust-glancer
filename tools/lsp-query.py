#!/usr/bin/env python3

"""Run bounded editor queries against rust-glancer's real LSP."""

import asyncio
from dataclasses import dataclass
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import traceback
from typing import Any, Callable, Dict, List, Optional, Sequence


MAX_FILE_BYTES = 2 * 1024 * 1024
MAX_QUERIES = 20
MAX_TIMEOUT_MS = 300_000
DEFAULT_TIMEOUT_MS = 180_000
MAX_HINTS = 200
MAX_COMPLETIONS = 200
MAX_CODE_ACTIONS = 100
STDERR_TAIL_BYTES = 64 * 1024
MAX_SERVER_LOG_BYTES = 64 * 1024 * 1024
MAX_PROTOCOL_MESSAGE_BYTES = 64 * 1024 * 1024
SHUTDOWN_TIMEOUT_MS = 5_000
PROCESS_EXIT_GRACE_MS = 2_000
MAX_QUEUED_NOTIFICATIONS = 64

ACTIVE_WORKSPACE_CHANGED = "rust-glancer/activeWorkspaceChanged"
DEFERRED_INDEXING_FINISHED = "rust-glancer/deferredIndexingFinished"
TRACKED_NOTIFICATIONS = {ACTIVE_WORKSPACE_CHANGED, DEFERRED_INDEXING_FINISHED}

TOOL_ROOT = Path(__file__).resolve().parent.parent


class LspQueryError(Exception):
    pass


@dataclass
class Options:
    profile: str = "release"
    timeout_ms: int = DEFAULT_TIMEOUT_MS
    json_output: bool = False
    show_logs: bool = False
    package_residency: str = "all-resident"
    max_hints: int = MAX_HINTS
    max_completions: int = MAX_COMPLETIONS
    max_code_actions: int = MAX_CODE_ACTIONS
    label: Optional[str] = None
    file: Optional[str] = None
    binary: Optional[str] = None
    query_file: Optional[str] = None
    query_json: Optional[str] = None
    overlay_file: Optional[str] = None
    workspace_root: Optional[str] = None
    command: Optional[str] = None
    marker: Optional[str] = None
    start_marker: Optional[str] = None
    end_marker: Optional[str] = None
    delta: int = 0
    occurrence: int = 1
    line: Optional[int] = None
    col: Optional[int] = None
    start_line: Optional[int] = None
    start_col: Optional[int] = None
    end_line: Optional[int] = None
    end_col: Optional[int] = None


def usage() -> str:
    return """
Usage:
  just agent-debug lsp-query --file <path> hover --marker <text> [--delta <n>] [--label <name>]
  just agent-debug lsp-query --file <path> hover --line <1-based> --col <1-based> [--label <name>]
  just agent-debug lsp-query --file <path> completion --marker <text> [--delta <n>] [--label <name>]
  just agent-debug lsp-query --file <path> completion --line <1-based> --col <1-based> [--label <name>]
  just agent-debug lsp-query --file <path> code-action --marker <text> [--delta <n>] [--label <name>]
  just agent-debug lsp-query --file <path> code-action --line <1-based> --col <1-based> [--label <name>]
  just agent-debug lsp-query --file <path> inlay --start-marker <text> --end-marker <text> [--label <name>]
  just agent-debug lsp-query --query-file <path> [--json]
  just agent-debug lsp-query --query-json <json> [--json]

Query file shape:
  {
    "file": "crates/example/src/lib.rs",
    "format": "text",
    "readinessBarrier": "ready",
    "deferredBarrier": "none",
    "initializationOptions": {"cache": {"packageResidency": "all-resident"}},
    "queries": [
      {"kind": "hover", "label": "local", "marker": "let value", "delta": 5},
      {"kind": "completion", "label": "member", "marker": "value.", "delta": 6,
       "context": {"triggerKind": 2, "triggerCharacter": "."}},
      {"kind": "code-action", "label": "fix", "marker": "MissingType",
       "context": {"triggerKind": 1, "only": ["quickfix"]}},
      {"kind": "inlay", "label": "block", "range": {"startMarker": "let value", "endMarker": "next_line"}}
    ]
  }

Notes:
  - Paths queried by LSP must stay inside --workspace-root (the repository by default).
  - --query-json avoids a plan file entirely.
  - agent-debug supplies its managed binary; direct invocation defaults to target/release/rust-glancer.
  - Use --workspace-root for an ad-hoc Cargo project under target/agent-debug/fixtures.
  - Set deferredBarrier to before-queries or after-queries when deferred indexing is relevant.
  - A plan may contain bounded inline "text" instead of an overlay file.
  - --line/--col are 1-based for human ergonomics.
""".strip()


def fail(message: str) -> None:
    raise LspQueryError(message)


def parse_integer(value: str, option: str) -> int:
    try:
        return int(value)
    except ValueError as error:
        raise LspQueryError("{} expects an integer".format(option)) from error


def parse_args(argv: Sequence[str]) -> Options:
    options = Options()
    value_options = {
        "--file": "file",
        "--binary": "binary",
        "--query-file": "query_file",
        "--query-json": "query_json",
        "--overlay-file": "overlay_file",
        "--workspace-root": "workspace_root",
        "--profile": "profile",
        "--package-residency": "package_residency",
        "--label": "label",
        "--marker": "marker",
        "--start-marker": "start_marker",
        "--end-marker": "end_marker",
    }
    integer_options = {
        "--timeout-ms": "timeout_ms",
        "--max-hints": "max_hints",
        "--max-completions": "max_completions",
        "--max-code-actions": "max_code_actions",
        "--delta": "delta",
        "--occurrence": "occurrence",
        "--line": "line",
        "--col": "col",
        "--start-line": "start_line",
        "--start-col": "start_col",
        "--end-line": "end_line",
        "--end-col": "end_col",
    }

    index = 0
    while index < len(argv):
        argument = argv[index]
        if argument in {"--help", "-h"}:
            print(usage())
            raise SystemExit(0)
        if argument in value_options or argument in integer_options:
            if index + 1 >= len(argv):
                fail("missing value for {}".format(argument))
            value = argv[index + 1]
            if argument in value_options:
                setattr(options, value_options[argument], value)
            else:
                setattr(options, integer_options[argument], parse_integer(value, argument))
            index += 2
            continue
        if argument == "--json":
            options.json_output = True
        elif argument == "--show-logs":
            options.show_logs = True
        elif argument.startswith("--"):
            fail("unknown option {}".format(argument))
        elif options.command is not None:
            fail("unexpected extra argument {}".format(argument))
        else:
            options.command = argument
        index += 1

    if options.timeout_ms <= 0 or options.timeout_ms > MAX_TIMEOUT_MS:
        fail("--timeout-ms must be between 1 and {}".format(MAX_TIMEOUT_MS))
    if options.max_hints <= 0 or options.max_hints > 5000:
        fail("--max-hints must be between 1 and 5000")
    if options.max_completions <= 0 or options.max_completions > 5000:
        fail("--max-completions must be between 1 and 5000")
    if options.max_code_actions <= 0 or options.max_code_actions > 5000:
        fail("--max-code-actions must be between 1 and 5000")
    if options.profile not in {"release", "debug"}:
        fail("--profile must be either release or debug")
    if options.query_file is not None and options.query_json is not None:
        fail("use only one of --query-file or --query-json")
    return options


def assert_inside(child: Path, parent: Path, label: str) -> Path:
    try:
        child.relative_to(parent)
    except ValueError:
        fail("{} must stay inside {}: {}".format(label, parent, child))
    return child


def assert_inside_any(child: Path, parents: Sequence[Path], label: str) -> Path:
    for parent in parents:
        try:
            child.relative_to(parent)
            return child
        except ValueError:
            continue
    fail("{} must stay inside one allowed root: {}".format(label, child))
    raise AssertionError("unreachable")


def workspace_root(options: Options) -> Path:
    root = (TOOL_ROOT / (options.workspace_root or ".")).resolve(strict=True)
    if not root.is_dir():
        fail("--workspace-root is not a directory: {}".format(root))
    if not (root / "Cargo.toml").exists():
        fail("--workspace-root does not contain Cargo.toml: {}".format(root))
    return root


def resolve_workspace_path(root: Path, value: Optional[str], label: str) -> Path:
    if not value:
        fail("{} is required".format(label))
    resolved = (root / value).resolve(strict=True)
    return assert_inside(resolved, root, label)


def resolve_readable_path(value: Optional[str], allowed_roots: Sequence[Path], label: str) -> Path:
    if not value:
        fail("{} is required".format(label))
    resolved = Path(value).resolve(strict=True)
    return assert_inside_any(resolved, allowed_roots, label)


def read_bounded_file(file_path: Path, label: str) -> str:
    stat = file_path.stat()
    if not file_path.is_file():
        fail("{} is not a file: {}".format(label, file_path))
    if stat.st_size > MAX_FILE_BYTES:
        fail("{} is too large: {} bytes, max {}".format(label, stat.st_size, MAX_FILE_BYTES))
    return file_path.read_text(encoding="utf-8")


def parse_query_json(raw: str, label: str) -> Any:
    if len(raw.encode("utf-8")) > MAX_FILE_BYTES:
        fail("{} is too large, max {} bytes".format(label, MAX_FILE_BYTES))
    try:
        return json.loads(raw)
    except json.JSONDecodeError as error:
        fail("invalid query JSON from {}: {}".format(label, error))
    raise AssertionError("unreachable")


def position_from_offset(text: str, offset: int) -> Dict[str, int]:
    line = text.count("\n", 0, offset)
    line_start = text.rfind("\n", 0, offset) + 1
    # LSP character offsets use UTF-16 code units, while Python indexes Unicode code points.
    character = len(text[line_start:offset].encode("utf-16-le")) // 2
    return {"line": line, "character": character}


def position_from_line_col(line: Any, col: Any) -> Dict[str, int]:
    if not isinstance(line, int) or isinstance(line, bool) or line <= 0:
        fail("line must be a positive 1-based integer")
    if not isinstance(col, int) or isinstance(col, bool) or col <= 0:
        fail("col must be a positive 1-based integer")
    return {"line": line - 1, "character": col - 1}


def position_from_marker(
    text: str, marker: Any, delta: Any = 0, occurrence: Any = 1
) -> Dict[str, int]:
    if not isinstance(marker, str) or not marker:
        fail("marker must be a non-empty string")
    if not isinstance(delta, int) or isinstance(delta, bool):
        fail("delta must be an integer")
    if not isinstance(occurrence, int) or isinstance(occurrence, bool) or occurrence <= 0:
        fail("occurrence must be a positive integer")

    search_from = 0
    offset = -1
    for _ in range(occurrence):
        offset = text.find(marker, search_from)
        if offset == -1:
            fail("marker not found: {}".format(marker))
        search_from = offset + len(marker)
    final_offset = offset + delta
    if final_offset < 0 or final_offset > len(text):
        fail("marker plus delta points outside file: marker={}, delta={}".format(marker, delta))
    return position_from_offset(text, final_offset)


def query_position(query: Dict[str, Any], text: str) -> Dict[str, int]:
    if query.get("position"):
        return position_from_line_col(query["position"].get("line"), query["position"].get("col"))
    if "line" in query or "col" in query:
        return position_from_line_col(query.get("line"), query.get("col"))
    return position_from_marker(
        text, query.get("marker"), query.get("delta", 0), query.get("occurrence", 1)
    )


def query_range(query: Dict[str, Any], text: str) -> Dict[str, Dict[str, int]]:
    query_range_value = query.get("range", query)
    if query_range_value.get("start") and query_range_value.get("end"):
        return {
            "start": position_from_line_col(
                query_range_value["start"].get("line"), query_range_value["start"].get("col")
            ),
            "end": position_from_line_col(
                query_range_value["end"].get("line"), query_range_value["end"].get("col")
            ),
        }
    if "startLine" in query_range_value or "endLine" in query_range_value:
        return {
            "start": position_from_line_col(
                query_range_value.get("startLine"), query_range_value.get("startCol")
            ),
            "end": position_from_line_col(
                query_range_value.get("endLine"), query_range_value.get("endCol")
            ),
        }
    return {
        "start": position_from_marker(
            text,
            query_range_value.get("startMarker"),
            query_range_value.get("startDelta", 0),
            query_range_value.get("startOccurrence", 1),
        ),
        "end": position_from_marker(
            text,
            query_range_value.get("endMarker"),
            query_range_value.get("endDelta", 0),
            query_range_value.get("endOccurrence", 1),
        ),
    }


def single_query_from_options(options: Options) -> Dict[str, Any]:
    if not options.command:
        fail("missing query command; expected hover, completion, code-action, or inlay")
    kind = "inlay" if options.command == "inlay-hints" else options.command
    if kind not in {"hover", "completion", "code-action", "inlay"}:
        fail("unsupported query command: {}".format(options.command))

    query: Dict[str, Any] = {
        "kind": kind,
        "label": options.label if options.label is not None else kind,
    }
    if kind in {"hover", "completion", "code-action"}:
        if options.marker is not None:
            query.update(
                {"marker": options.marker, "delta": options.delta, "occurrence": options.occurrence}
            )
        else:
            query.update({"line": options.line, "col": options.col})
    elif options.start_marker is not None or options.end_marker is not None:
        query["range"] = {
            "startMarker": options.start_marker,
            "endMarker": options.end_marker,
        }
    else:
        query["range"] = {
            "startLine": options.start_line,
            "startCol": options.start_col,
            "endLine": options.end_line,
            "endCol": options.end_col,
        }
    return query


def load_query_plan(options: Options, root: Path) -> Dict[str, Any]:
    allowed_roots = [TOOL_ROOT, root, Path(tempfile.gettempdir()).resolve()]
    if options.query_file is not None:
        if options.query_file == "-":
            fail("--query-file - is unsupported; use --query-json or a plan file")
        raw = read_bounded_file(
            resolve_readable_path(options.query_file, allowed_roots, "--query-file"),
            "--query-file",
        )
        return normalize_plan(parse_query_json(raw, "--query-file"), root, options)
    if options.query_json is not None:
        return normalize_plan(parse_query_json(options.query_json, "--query-json"), root, options)
    if not options.file:
        fail("--file is required without --query-file")
    return normalize_plan(
        {
            "file": options.file,
            "overlayFile": options.overlay_file,
            "queries": [single_query_from_options(options)],
        },
        root,
        options,
    )


def require_object(value: Any, label: str) -> Dict[str, Any]:
    if not isinstance(value, dict):
        fail("{} must be an object".format(label))
    return value


def normalize_plan(plan_value: Any, root: Path, options: Options) -> Dict[str, Any]:
    plan = require_object(plan_value, "query plan")
    file_path = resolve_workspace_path(root, plan.get("file", options.file), "file")
    allowed_overlay_roots = [TOOL_ROOT, root, Path(tempfile.gettempdir()).resolve()]
    overlay_file = plan.get("overlayFile", options.overlay_file)
    if "text" in plan and plan["text"] is not None and overlay_file is not None:
        fail("query plan must use only one of text or overlayFile")
    if "text" in plan and plan["text"] is not None and not isinstance(plan["text"], str):
        fail("query plan text must be a string")
    if isinstance(plan.get("text"), str) and len(plan["text"].encode("utf-8")) > MAX_FILE_BYTES:
        fail("query plan text is too large, max {} bytes".format(MAX_FILE_BYTES))
    if plan.get("text") is not None:
        text = plan["text"]
    elif overlay_file is not None:
        text = read_bounded_file(
            resolve_readable_path(overlay_file, allowed_overlay_roots, "overlayFile"),
            "overlayFile",
        )
    else:
        text = read_bounded_file(file_path, "file")

    query_values = plan.get("queries")
    if not isinstance(query_values, list) or not query_values:
        fail("query plan must contain queries")
    if len(query_values) > MAX_QUERIES:
        fail("too many queries: {}, max {}".format(len(query_values), MAX_QUERIES))
    queries = []
    for query_value in query_values:
        query = dict(require_object(query_value, "each query"))
        if query.get("kind") == "codeAction":
            query["kind"] = "code-action"
        if query.get("kind") not in {
            "hover",
            "completion",
            "code-action",
            "inlay",
            "inlay-hints",
        }:
            fail("unsupported query kind: {}".format(query.get("kind")))
        if query["kind"] == "inlay-hints":
            query["kind"] = "inlay"
        if "context" in query and query["context"] is not None:
            context = require_object(query["context"], "query context")
            if query["kind"] == "completion":
                if context.get("triggerKind") not in {1, 2, 3}:
                    fail("completion context triggerKind must be 1, 2, or 3")
                trigger_character = context.get("triggerCharacter")
                if trigger_character is not None and not isinstance(trigger_character, str):
                    fail("completion context triggerCharacter must be a string")
            elif query["kind"] == "code-action":
                if context.get("triggerKind") not in {None, 1, 2}:
                    fail("code-action context triggerKind must be 1 or 2")
                only = context.get("only")
                if only is not None and (
                    not isinstance(only, list)
                    or any(not isinstance(kind, str) for kind in only)
                ):
                    fail("code-action context only must be an array of strings")
                diagnostics = context.get("diagnostics")
                if diagnostics is not None and not isinstance(diagnostics, list):
                    fail("code-action context diagnostics must be an array")
            else:
                fail("query context is only supported for completion and code-action")
        queries.append(query)

    output_format = plan.get("format", "text")
    if output_format not in {"text", "json"}:
        fail("query plan format must be text or json")
    readiness_barrier = plan.get("readinessBarrier", "ready")
    if readiness_barrier not in {"ready", "none"}:
        fail("readinessBarrier must be ready or none")
    deferred_barrier = plan.get("deferredBarrier", "none")
    if deferred_barrier not in {"none", "before-queries", "after-queries"}:
        fail("deferredBarrier must be none, before-queries, or after-queries")
    if "cfgTest" in plan and plan["cfgTest"] is not None and not isinstance(plan["cfgTest"], bool):
        fail("cfgTest must be a boolean")

    requested_value = plan.get("initializationOptions")
    requested = {} if requested_value is None else require_object(
        requested_value, "initializationOptions"
    )
    for key in ("cache", "cfg", "diagnostics"):
        if key in requested and requested[key] is not None and not isinstance(requested[key], dict):
            fail("initializationOptions.{} must be an object".format(key))
    requested_cfg = requested.get("cfg") or {}
    if "cfgTest" in plan and plan["cfgTest"] is not None and "test" in requested_cfg:
        fail("use only one of cfgTest or initializationOptions.cfg.test")

    initialization_options = dict(requested)
    cache = {"packageResidency": options.package_residency}
    cache.update(requested.get("cache") or {})
    cfg = dict(requested_cfg)
    if "cfgTest" in plan and plan["cfgTest"] is not None:
        cfg["test"] = plan["cfgTest"]
    diagnostics = {"onStartup": False, "onSave": False}
    diagnostics.update(requested.get("diagnostics") or {})
    initialization_options.update({"cache": cache, "cfg": cfg, "diagnostics": diagnostics})

    return {
        "file": file_path,
        "text": text,
        "queries": queries,
        "format": output_format,
        "initializationOptions": initialization_options,
        "readinessBarrier": readiness_barrier,
        "deferredBarrier": deferred_barrier,
    }


def release_binary(options: Options) -> Path:
    candidate = (TOOL_ROOT / (options.binary or "target/{}/rust-glancer".format(options.profile))).resolve()
    if not candidate.exists():
        fail("{} does not exist. Build it first or use just agent-debug lsp-query".format(candidate))
    binary = candidate.resolve(strict=True)
    if not binary.is_file():
        fail("--binary is not a file: {}".format(binary))
    return binary


class LspClient:
    def __init__(
        self,
        process: "asyncio.subprocess.Process",
        timeout_ms: int,
        show_logs: bool,
    ) -> None:
        self.process = process
        self.timeout_ms = timeout_ms
        self.show_logs = show_logs
        self.next_id = 1
        self.pending: Dict[int, "asyncio.Future[Dict[str, Any]]"] = {}
        self.notifications: List[Dict[str, Any]] = []
        self.notification_waiters: List[Dict[str, Any]] = []
        self.stderr_tail = bytearray()
        self.exited = False
        self.protocol_error: Optional[Exception] = None
        output_directory = os.environ.get("RUST_GLANCER_AGENT_DEBUG_OUTPUT_DIR")
        self.server_log_file = (
            Path(output_directory) / "lsp-server.stderr.log" if output_directory else None
        )
        self.server_log_bytes = 0
        self.server_log_truncated = False
        self.stdout_task = asyncio.create_task(self._read_stdout())
        self.stderr_task = asyncio.create_task(self._read_stderr())
        self.exit_task = asyncio.create_task(self._watch_exit())

    @classmethod
    async def start(
        cls, binary: Path, root: Path, timeout_ms: int, show_logs: bool
    ) -> "LspClient":
        process = await asyncio.create_subprocess_exec(
            str(binary),
            "lsp",
            cwd=str(root),
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        return cls(process, timeout_ms, show_logs)

    @property
    def stderr(self) -> str:
        return bytes(self.stderr_tail).decode("utf-8", errors="replace")

    def _fail_waiting(self, error: Exception) -> None:
        for future in self.pending.values():
            if not future.done():
                future.set_exception(error)
        self.pending.clear()
        for waiter in self.notification_waiters:
            future = waiter["future"]
            if not future.done():
                future.set_exception(error)
        self.notification_waiters.clear()

    async def _watch_exit(self) -> int:
        return_code = await self.process.wait()
        try:
            await self.stdout_task
        except Exception as error:
            self.protocol_error = error
        self.exited = True
        error = self.protocol_error or RuntimeError(
            "LSP process exited before responding: returncode={}".format(return_code)
        )
        self._fail_waiting(error)
        return return_code

    async def _read_stdout(self) -> None:
        assert self.process.stdout is not None
        try:
            while True:
                header = await self.process.stdout.readuntil(b"\r\n\r\n")
                content_length = None
                for line in header[:-4].split(b"\r\n"):
                    name, separator, value = line.partition(b":")
                    if separator and name.lower() == b"content-length":
                        content_length = int(value.strip())
                if content_length is None:
                    raise RuntimeError("LSP message is missing Content-Length")
                if content_length < 0 or content_length > MAX_PROTOCOL_MESSAGE_BYTES:
                    raise RuntimeError("invalid LSP Content-Length {}".format(content_length))
                body = await self.process.stdout.readexactly(content_length)
                message = json.loads(body.decode("utf-8"))
                if not isinstance(message, dict):
                    raise RuntimeError("LSP message must be a JSON object")
                await self._on_message(message)
        except asyncio.IncompleteReadError as error:
            if error.partial:
                self.protocol_error = RuntimeError("LSP stdout ended in the middle of a message")
                self._fail_waiting(self.protocol_error)
        except Exception as error:
            self.protocol_error = error
            self._fail_waiting(error)

    async def _read_stderr(self) -> None:
        assert self.process.stderr is not None
        log_file = None
        try:
            if self.server_log_file is not None:
                log_file = self.server_log_file.open("ab")
            while True:
                chunk = await self.process.stderr.read(64 * 1024)
                if not chunk:
                    return
                self.stderr_tail.extend(chunk)
                if len(self.stderr_tail) > STDERR_TAIL_BYTES:
                    del self.stderr_tail[: len(self.stderr_tail) - STDERR_TAIL_BYTES]
                if log_file is not None:
                    remaining = max(0, MAX_SERVER_LOG_BYTES - self.server_log_bytes)
                    if remaining > 0:
                        retained = chunk[:remaining]
                        log_file.write(retained)
                        log_file.flush()
                        self.server_log_bytes += len(retained)
                    if len(chunk) > remaining and not self.server_log_truncated:
                        log_file.write(
                            "\n<lsp-query server log truncated after {} bytes>\n".format(
                                MAX_SERVER_LOG_BYTES
                            ).encode("utf-8")
                        )
                        log_file.flush()
                        self.server_log_truncated = True
                if self.show_logs:
                    sys.stderr.buffer.write(chunk)
                    sys.stderr.buffer.flush()
        finally:
            if log_file is not None:
                log_file.close()

    async def _on_message(self, message: Dict[str, Any]) -> None:
        if "id" in message and message["id"] in self.pending:
            future = self.pending.pop(message["id"])
            if not future.done():
                future.set_result(message)
            return

        if "id" not in message and message.get("method"):
            for index, waiter in enumerate(self.notification_waiters):
                if waiter["predicate"](message):
                    self.notification_waiters.pop(index)
                    future = waiter["future"]
                    if not future.done():
                        future.set_result(message)
                    return
            if message["method"] in TRACKED_NOTIFICATIONS:
                self.notifications.append(message)
                if len(self.notifications) > MAX_QUEUED_NOTIFICATIONS:
                    self.notifications.pop(0)
            return

        # A minimal client still needs to acknowledge server-to-client requests made during
        # initialization. Returning null keeps the harness deterministic.
        if "id" in message and message.get("method"):
            await self.send({"jsonrpc": "2.0", "id": message["id"], "result": None})

    async def send(self, message: Dict[str, Any]) -> None:
        if self.process.stdin is None or self.process.stdin.is_closing():
            raise RuntimeError("LSP stdin is closed")
        body = json.dumps(message, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
        self.process.stdin.write(
            "Content-Length: {}\r\n\r\n".format(len(body)).encode("ascii") + body
        )
        await self.process.stdin.drain()

    async def request(
        self, method: str, params: Any, timeout_ms: Optional[int] = None
    ) -> Dict[str, Any]:
        if self.exited:
            raise RuntimeError("LSP process exited before {}".format(method))
        if self.protocol_error is not None:
            raise self.protocol_error
        request_id = self.next_id
        self.next_id += 1
        future = asyncio.get_running_loop().create_future()
        self.pending[request_id] = future
        try:
            await self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
            response = await asyncio.wait_for(
                asyncio.shield(future), (timeout_ms or self.timeout_ms) / 1000
            )
            # JSON-RPC transports method failures as ordinary responses. Turn them into CLI
            # failures here so no caller can accidentally interpret a missing result as success.
            if response.get("error") is not None:
                raise LspQueryError(
                    "{} failed: {}".format(
                        method, json.dumps(response["error"], ensure_ascii=False)
                    )
                )
            return response
        except asyncio.TimeoutError as error:
            self.pending.pop(request_id, None)
            future.cancel()
            raise RuntimeError("timeout waiting for {}".format(method)) from error
        except Exception:
            self.pending.pop(request_id, None)
            future.cancel()
            raise

    async def notify(self, method: str, params: Any) -> None:
        await self.send({"jsonrpc": "2.0", "method": method, "params": params})

    async def wait_for_notification(
        self,
        predicate: Callable[[Dict[str, Any]], bool],
        description: str,
        timeout_ms: Optional[int] = None,
    ) -> Dict[str, Any]:
        for index, message in enumerate(self.notifications):
            if predicate(message):
                return self.notifications.pop(index)
        if self.exited:
            raise RuntimeError("LSP process exited before {}".format(description))

        future = asyncio.get_running_loop().create_future()
        waiter = {"predicate": predicate, "future": future}
        self.notification_waiters.append(waiter)
        try:
            return await asyncio.wait_for(
                asyncio.shield(future), (timeout_ms or self.timeout_ms) / 1000
            )
        except asyncio.TimeoutError as error:
            if waiter in self.notification_waiters:
                self.notification_waiters.remove(waiter)
            future.cancel()
            raise RuntimeError("timeout waiting for {}".format(description)) from error

    async def wait_for_exit(self, timeout_ms: int) -> bool:
        if self.exited:
            return True
        try:
            await asyncio.wait_for(asyncio.shield(self.exit_task), timeout_ms / 1000)
            return True
        except asyncio.TimeoutError:
            return False

    async def close(self) -> None:
        if not self.exited:
            try:
                await self.request("shutdown", None, min(self.timeout_ms, SHUTDOWN_TIMEOUT_MS))
                await self.notify("exit", None)
            except Exception:
                pass
            if not await self.wait_for_exit(PROCESS_EXIT_GRACE_MS):
                try:
                    self.process.terminate()
                except ProcessLookupError:
                    pass
                if not await self.wait_for_exit(PROCESS_EXIT_GRACE_MS):
                    try:
                        self.process.kill()
                    except ProcessLookupError:
                        pass
                    await self.exit_task
        await asyncio.gather(self.stdout_task, self.stderr_task, return_exceptions=True)


async def wait_until_ready(client: LspClient, timeout_ms: int) -> None:
    notification = await client.wait_for_notification(
        lambda message: message.get("method") == ACTIVE_WORKSPACE_CHANGED
        and (message.get("params") or {}).get("state") in {"ready", "failed"},
        "rust-glancer workspace readiness",
        timeout_ms,
    )
    params = notification.get("params") or {}
    if params.get("state") == "failed":
        fail("workspace indexing failed: {}".format(params.get("message", "no reason reported")))


async def wait_until_deferred_indexing_finishes(client: LspClient, timeout_ms: int) -> None:
    notification = await client.wait_for_notification(
        lambda message: message.get("method") == DEFERRED_INDEXING_FINISHED
        or (
            message.get("method") == ACTIVE_WORKSPACE_CHANGED
            and (message.get("params") or {}).get("state") == "failed"
        ),
        "rust-glancer deferred indexing",
        timeout_ms,
    )
    if notification.get("method") == ACTIVE_WORKSPACE_CHANGED:
        params = notification.get("params") or {}
        fail("workspace indexing failed: {}".format(params.get("message", "no reason reported")))


def hover_text(result: Any) -> Optional[str]:
    if not result or not isinstance(result, dict):
        return None
    contents = result.get("contents")
    if not contents:
        return None
    if isinstance(contents, str):
        return contents
    if isinstance(contents, dict) and isinstance(contents.get("value"), str):
        return contents["value"]
    if isinstance(contents, list):
        values = []
        for item in contents:
            if isinstance(item, str):
                values.append(item)
            elif isinstance(item, dict) and item.get("value"):
                values.append(item["value"])
        return "\n".join(values)
    return json.dumps(contents, ensure_ascii=False)


def normalize_hints(hints_value: Any, max_hints: int) -> List[Dict[str, Any]]:
    hints = hints_value if isinstance(hints_value, list) else []
    normalized = []
    for hint in hints[:max_hints]:
        position = hint.get("position") or {}
        label = hint.get("label")
        normalized.append(
            {
                "line": position.get("line", 0) + 1,
                "col": position.get("character", 0) + 1,
                "kind": hint.get("kind"),
                "label": label if isinstance(label, str) else json.dumps(label, ensure_ascii=False),
            }
        )
    return normalized


def normalize_completions(
    completions_value: Any, max_completions: int
) -> Dict[str, Any]:
    if completions_value is None:
        raw_items = []
        is_incomplete = False
    elif isinstance(completions_value, list):
        raw_items = completions_value
        is_incomplete = False
    elif isinstance(completions_value, dict) and isinstance(
        completions_value.get("items"), list
    ):
        raw_items = completions_value["items"]
        is_incomplete = bool(completions_value.get("isIncomplete", False))
    else:
        fail("completion result must be an array, completion list, or null")

    items = []
    for raw_item in raw_items[:max_completions]:
        if not isinstance(raw_item, dict):
            fail("each completion item must be an object")
        label = raw_item.get("label")
        items.append(
            {
                "label": label
                if isinstance(label, str)
                else json.dumps(label, ensure_ascii=False),
                "kind": raw_item.get("kind"),
                "detail": raw_item.get("detail"),
                "sortText": raw_item.get("sortText"),
                "filterText": raw_item.get("filterText"),
                "insertText": raw_item.get("insertText"),
            }
        )

    return {
        "items": items,
        "totalCount": len(raw_items),
        "isIncomplete": is_incomplete,
        "truncated": len(raw_items) > max_completions,
    }


def normalize_code_actions(actions_value: Any, max_actions: int) -> Dict[str, Any]:
    if actions_value is None:
        raw_actions = []
    elif isinstance(actions_value, list):
        raw_actions = actions_value
    else:
        fail("code-action result must be an array or null")

    actions = []
    for raw_action in raw_actions[:max_actions]:
        if not isinstance(raw_action, dict):
            fail("each code action must be an object")
        document_changes = (raw_action.get("edit") or {}).get("documentChanges") or []
        edit_count = 0
        versions = []
        for document_change in document_changes:
            if not isinstance(document_change, dict):
                continue
            edits = document_change.get("edits")
            if isinstance(edits, list):
                edit_count += len(edits)
            text_document = document_change.get("textDocument")
            if isinstance(text_document, dict):
                versions.append(text_document.get("version"))
        actions.append(
            {
                "title": raw_action.get("title"),
                "kind": raw_action.get("kind"),
                "preferred": raw_action.get("isPreferred"),
                "editCount": edit_count,
                "documentVersions": versions,
                "disabled": raw_action.get("disabled"),
            }
        )

    return {
        "actions": actions,
        "totalCount": len(raw_actions),
        "truncated": len(raw_actions) > max_actions,
    }


def without_none(value: Any) -> Any:
    if isinstance(value, dict):
        return {key: without_none(item) for key, item in value.items() if item is not None}
    if isinstance(value, list):
        return [without_none(item) for item in value]
    return value


async def run(argv: Sequence[str]) -> None:
    options = parse_args(argv)
    root = workspace_root(options)
    plan = load_query_plan(options, root)
    binary = release_binary(options)
    file_path = plan["file"]
    uri = file_path.as_uri()
    client = await LspClient.start(binary, root, options.timeout_ms, options.show_logs)
    results = []

    try:
        await client.request(
            "initialize",
            {
                "processId": os.getpid(),
                "rootUri": root.as_uri(),
                "workspaceFolders": [{"uri": root.as_uri(), "name": root.name}],
                "capabilities": {
                    "workspace": {"workspaceEdit": {"documentChanges": True}},
                    "textDocument": {
                        "hover": {"contentFormat": ["markdown", "plaintext"]},
                        "completion": {
                            "dynamicRegistration": False,
                            "completionItem": {"snippetSupport": True},
                        },
                        "codeAction": {
                            "codeActionLiteralSupport": {
                                "codeActionKind": {
                                    "valueSet": ["quickfix", "refactor.rewrite"]
                                }
                            },
                            "isPreferredSupport": True,
                        },
                        "inlayHint": {"dynamicRegistration": False},
                    }
                },
                "initializationOptions": plan["initializationOptions"],
            },
        )
        await client.notify("initialized", {})
        await client.notify(
            "textDocument/didOpen",
            {
                "textDocument": {
                    "uri": uri,
                    "languageId": "rust",
                    "version": 1,
                    "text": plan["text"],
                }
            },
        )

        # Normal editor queries can start once the workspace is ready. Plans that need settled
        # body indexes opt into the stronger deferred barrier instead of inventing sleeps.
        if plan["readinessBarrier"] == "ready":
            await wait_until_ready(client, options.timeout_ms)
        if plan["deferredBarrier"] == "before-queries":
            await wait_until_deferred_indexing_finishes(client, options.timeout_ms)

        for query in plan["queries"]:
            if query["kind"] == "hover":
                position = query_position(query, plan["text"])
                request_started = time.perf_counter_ns()
                response = await client.request(
                    "textDocument/hover", {"textDocument": {"uri": uri}, "position": position}
                )
                elapsed_ms = (time.perf_counter_ns() - request_started) / 1_000_000
                results.append(
                    {
                        "kind": "hover",
                        "label": query.get("label")
                        if query.get("label") is not None
                        else "hover",
                        "position": {
                            "line": position["line"] + 1,
                            "col": position["character"] + 1,
                        },
                        "elapsedMs": round(elapsed_ms, 3),
                        "text": hover_text(response.get("result")),
                        "raw": response.get("result"),
                    }
                )
            elif query["kind"] == "completion":
                position = query_position(query, plan["text"])
                params = {"textDocument": {"uri": uri}, "position": position}
                if query.get("context") is not None:
                    params["context"] = query["context"]
                request_started = time.perf_counter_ns()
                response = await client.request("textDocument/completion", params)
                elapsed_ms = (time.perf_counter_ns() - request_started) / 1_000_000
                completions = normalize_completions(
                    response.get("result"), options.max_completions
                )
                results.append(
                    {
                        "kind": "completion",
                        "label": query.get("label")
                        if query.get("label") is not None
                        else "completion",
                        "position": {
                            "line": position["line"] + 1,
                            "col": position["character"] + 1,
                        },
                        "elapsedMs": round(elapsed_ms, 3),
                        **completions,
                    }
                )
            elif query["kind"] == "code-action":
                if query.get("range") is not None:
                    query_range_value = query_range(query, plan["text"])
                else:
                    position = query_position(query, plan["text"])
                    query_range_value = {"start": position, "end": position}
                context = {"diagnostics": [], "triggerKind": 1}
                context.update(query.get("context") or {})
                request_started = time.perf_counter_ns()
                response = await client.request(
                    "textDocument/codeAction",
                    {
                        "textDocument": {"uri": uri},
                        "range": query_range_value,
                        "context": context,
                    },
                )
                elapsed_ms = (time.perf_counter_ns() - request_started) / 1_000_000
                actions = normalize_code_actions(
                    response.get("result"), options.max_code_actions
                )
                results.append(
                    {
                        "kind": "code-action",
                        "label": query.get("label")
                        if query.get("label") is not None
                        else "code-action",
                        "range": {
                            "start": {
                                "line": query_range_value["start"]["line"] + 1,
                                "col": query_range_value["start"]["character"] + 1,
                            },
                            "end": {
                                "line": query_range_value["end"]["line"] + 1,
                                "col": query_range_value["end"]["character"] + 1,
                            },
                        },
                        "elapsedMs": round(elapsed_ms, 3),
                        **actions,
                    }
                )
            elif query["kind"] == "inlay":
                query_range_value = query_range(query, plan["text"])
                request_started = time.perf_counter_ns()
                response = await client.request(
                    "textDocument/inlayHint",
                    {"textDocument": {"uri": uri}, "range": query_range_value},
                )
                elapsed_ms = (time.perf_counter_ns() - request_started) / 1_000_000
                raw_hints = response.get("result")
                results.append(
                    {
                        "kind": "inlay",
                        "label": query.get("label")
                        if query.get("label") is not None
                        else "inlay",
                        "range": {
                            "start": {
                                "line": query_range_value["start"]["line"] + 1,
                                "col": query_range_value["start"]["character"] + 1,
                            },
                            "end": {
                                "line": query_range_value["end"]["line"] + 1,
                                "col": query_range_value["end"]["character"] + 1,
                            },
                        },
                        "elapsedMs": round(elapsed_ms, 3),
                        "hints": normalize_hints(raw_hints, options.max_hints),
                        "truncated": isinstance(raw_hints, list)
                        and len(raw_hints) > options.max_hints,
                        "raw": raw_hints,
                    }
                )
        if plan["deferredBarrier"] == "after-queries":
            await wait_until_deferred_indexing_finishes(client, options.timeout_ms)
    except Exception:
        if not options.show_logs and client.stderr.strip():
            print("lsp-query: server stderr tail:\n{}".format(client.stderr), file=sys.stderr)
        raise
    finally:
        await client.close()

    output = {
        "file": os.path.relpath(str(file_path), str(root)),
        "binary": os.path.relpath(str(binary), str(TOOL_ROOT)),
        "barriers": {
            "readiness": plan["readinessBarrier"],
            "deferred": plan["deferredBarrier"],
        },
        "results": results,
    }

    if options.json_output or plan["format"] == "json":
        print(json.dumps(without_none(output), indent=2, ensure_ascii=False))
        return

    print("file: {}".format(output["file"]))
    print("binary: {}".format(output["binary"]))
    for result in results:
        label = " " + str(result["label"])
        if result["kind"] == "hover":
            print(
                "\nhover{} @ {}:{} ({:.3f} ms)".format(
                    label,
                    result["position"]["line"],
                    result["position"]["col"],
                    result["elapsedMs"],
                )
            )
            print(result.get("text") or "<no hover>")
        elif result["kind"] == "completion":
            print(
                "\ncompletion{} @ {}:{} ({:.3f} ms, {} items)".format(
                    label,
                    result["position"]["line"],
                    result["position"]["col"],
                    result["elapsedMs"],
                    result["totalCount"],
                )
            )
            for item in result["items"]:
                detail = " — {}".format(item["detail"]) if item.get("detail") else ""
                print("  {}{}".format(item["label"], detail))
            if result["truncated"]:
                print("  <truncated>")
        elif result["kind"] == "code-action":
            print(
                "\ncode-action{} @ {}:{}..{}:{} ({:.3f} ms, {} actions)".format(
                    label,
                    result["range"]["start"]["line"],
                    result["range"]["start"]["col"],
                    result["range"]["end"]["line"],
                    result["range"]["end"]["col"],
                    result["elapsedMs"],
                    result["totalCount"],
                )
            )
            for action in result["actions"]:
                preferred = " preferred" if action.get("preferred") else ""
                versions = ",".join(
                    "none" if version is None else str(version)
                    for version in action["documentVersions"]
                )
                version_detail = " versions={}".format(versions) if versions else ""
                print(
                    "  {} [{}]{} edits={}{}".format(
                        action.get("title") or "<untitled>",
                        action.get("kind") or "<none>",
                        preferred,
                        action["editCount"],
                        version_detail,
                    )
                )
            if result["truncated"]:
                print("  <truncated>")
        else:
            print(
                "\ninlay{} @ {}:{}..{}:{} ({:.3f} ms)".format(
                    label,
                    result["range"]["start"]["line"],
                    result["range"]["start"]["col"],
                    result["range"]["end"]["line"],
                    result["range"]["end"]["col"],
                    result["elapsedMs"],
                )
            )
            for hint in result["hints"]:
                print("  {}:{} {}".format(hint["line"], hint["col"], hint["label"]))
            if result["truncated"]:
                print("  <truncated>")


def main() -> int:
    try:
        asyncio.run(run(sys.argv[1:]))
        return 0
    except LspQueryError as error:
        print("lsp-query: {}".format(error), file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        return 130
    except SystemExit as error:
        return int(error.code or 0)
    except Exception as error:
        print("lsp-query: {}".format(error), file=sys.stderr)
        traceback.print_exc()
        return 1


if __name__ == "__main__":
    sys.exit(main())
