#!/usr/bin/env node

import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";

const MAX_FILE_BYTES = 2 * 1024 * 1024;
const MAX_QUERIES = 20;
const MAX_TIMEOUT_MS = 300_000;
const DEFAULT_TIMEOUT_MS = 180_000;
const MAX_HINTS = 200;
const STDERR_TAIL_BYTES = 64 * 1024;

function usage() {
  return `
Usage:
  just lsp-query <query-file>
  node tools/lsp-query.mjs --file <path> hover --marker <text> [--delta <n>] [--label <name>]
  node tools/lsp-query.mjs --file <path> hover --line <1-based> --col <1-based> [--label <name>]
  node tools/lsp-query.mjs --file <path> inlay --start-marker <text> --end-marker <text> [--label <name>]
  node tools/lsp-query.mjs --query-file <path> [--json]

Query file shape:
  {
    "file": "crates/example/src/lib.rs",
    "format": "text",
    "queries": [
      {"kind": "hover", "label": "local", "marker": "let value", "delta": 5},
      {"kind": "inlay", "label": "block", "range": {"startMarker": "let value", "endMarker": "next_line"}}
    ]
  }

Notes:
  - Run from the rust-glancer workspace root.
  - Uses target/release/rust-glancer by default. Pass --profile debug only when needed.
  - Paths queried by LSP must stay inside the workspace.
  - --line/--col are 1-based for human ergonomics.
`.trim();
}

function fail(message) {
  console.error(`lsp-query: ${message}`);
  process.exit(2);
}

function parseArgs(argv) {
  const options = {
    profile: "release",
    timeoutMs: DEFAULT_TIMEOUT_MS,
    json: false,
    showLogs: false,
    packageResidency: "all-resident",
    maxHints: MAX_HINTS,
    label: undefined,
    file: undefined,
    queryFile: undefined,
    overlayFile: undefined,
    command: undefined,
    marker: undefined,
    startMarker: undefined,
    endMarker: undefined,
    delta: 0,
    occurrence: 1,
    line: undefined,
    col: undefined,
    startLine: undefined,
    startCol: undefined,
    endLine: undefined,
    endCol: undefined,
  };

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    const next = () => {
      i += 1;
      if (i >= argv.length) fail(`missing value for ${arg}`);
      return argv[i];
    };

    switch (arg) {
      case "--help":
      case "-h":
        console.log(usage());
        process.exit(0);
      case "--file":
        options.file = next();
        break;
      case "--query-file":
        options.queryFile = next();
        break;
      case "--overlay-file":
        options.overlayFile = next();
        break;
      case "--profile":
        options.profile = next();
        break;
      case "--timeout-ms":
        options.timeoutMs = Number(next());
        break;
      case "--json":
        options.json = true;
        break;
      case "--show-logs":
        options.showLogs = true;
        break;
      case "--package-residency":
        options.packageResidency = next();
        break;
      case "--max-hints":
        options.maxHints = Number(next());
        break;
      case "--label":
        options.label = next();
        break;
      case "--marker":
        options.marker = next();
        break;
      case "--start-marker":
        options.startMarker = next();
        break;
      case "--end-marker":
        options.endMarker = next();
        break;
      case "--delta":
        options.delta = Number(next());
        break;
      case "--occurrence":
        options.occurrence = Number(next());
        break;
      case "--line":
        options.line = Number(next());
        break;
      case "--col":
        options.col = Number(next());
        break;
      case "--start-line":
        options.startLine = Number(next());
        break;
      case "--start-col":
        options.startCol = Number(next());
        break;
      case "--end-line":
        options.endLine = Number(next());
        break;
      case "--end-col":
        options.endCol = Number(next());
        break;
      default:
        if (arg.startsWith("--")) {
          fail(`unknown option ${arg}`);
        }
        if (options.command !== undefined) {
          fail(`unexpected extra argument ${arg}`);
        }
        options.command = arg;
        break;
    }
  }

  if (!Number.isInteger(options.timeoutMs) || options.timeoutMs <= 0 || options.timeoutMs > MAX_TIMEOUT_MS) {
    fail(`--timeout-ms must be between 1 and ${MAX_TIMEOUT_MS}`);
  }
  if (!Number.isInteger(options.maxHints) || options.maxHints <= 0 || options.maxHints > 5000) {
    fail("--max-hints must be between 1 and 5000");
  }
  if (!["release", "debug"].includes(options.profile)) {
    fail("--profile must be either release or debug");
  }

  return options;
}

function assertInside(child, parent, label) {
  const relative = path.relative(parent, child);
  if (relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative))) {
    return child;
  }
  fail(`${label} must stay inside ${parent}: ${child}`);
}

function assertInsideAny(child, parents, label) {
  for (const parent of parents) {
    const relative = path.relative(parent, child);
    if (relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative))) {
      return child;
    }
  }
  fail(`${label} must stay inside one allowed root: ${child}`);
}

function workspaceRoot() {
  return fs.realpathSync(process.cwd());
}

function resolveWorkspacePath(root, value, label) {
  if (!value) fail(`${label} is required`);
  const resolved = fs.realpathSync(path.resolve(root, value));
  return assertInside(resolved, root, label);
}

function resolveReadablePath(value, allowedRoots, label) {
  if (!value) fail(`${label} is required`);
  const resolved = fs.realpathSync(path.resolve(value));
  return assertInsideAny(resolved, allowedRoots, label);
}

function readBoundedFile(filePath, label) {
  const stat = fs.statSync(filePath);
  if (!stat.isFile()) fail(`${label} is not a file: ${filePath}`);
  if (stat.size > MAX_FILE_BYTES) {
    fail(`${label} is too large: ${stat.size} bytes, max ${MAX_FILE_BYTES}`);
  }
  return fs.readFileSync(filePath, "utf8");
}

function positionFromOffset(text, offset) {
  let line = 0;
  let lineStart = 0;
  for (let i = 0; i < offset; i += 1) {
    if (text.charCodeAt(i) === 10) {
      line += 1;
      lineStart = i + 1;
    }
  }
  return { line, character: offset - lineStart };
}

function positionFromLineCol(line, col) {
  if (!Number.isInteger(line) || line <= 0) fail("line must be a positive 1-based integer");
  if (!Number.isInteger(col) || col <= 0) fail("col must be a positive 1-based integer");
  return { line: line - 1, character: col - 1 };
}

function positionFromMarker(text, marker, delta = 0, occurrence = 1) {
  if (typeof marker !== "string" || marker.length === 0) fail("marker must be a non-empty string");
  if (!Number.isInteger(delta)) fail("delta must be an integer");
  if (!Number.isInteger(occurrence) || occurrence <= 0) {
    fail("occurrence must be a positive integer");
  }

  let searchFrom = 0;
  let offset = -1;
  for (let seen = 0; seen < occurrence; seen += 1) {
    offset = text.indexOf(marker, searchFrom);
    if (offset === -1) fail(`marker not found: ${marker}`);
    searchFrom = offset + marker.length;
  }

  const finalOffset = offset + delta;
  if (finalOffset < 0 || finalOffset > text.length) {
    fail(`marker plus delta points outside file: marker=${marker}, delta=${delta}`);
  }
  return positionFromOffset(text, finalOffset);
}

function queryPosition(query, text) {
  if (query.position) {
    return positionFromLineCol(query.position.line, query.position.col);
  }
  if (query.line !== undefined || query.col !== undefined) {
    return positionFromLineCol(query.line, query.col);
  }
  return positionFromMarker(text, query.marker, query.delta ?? 0, query.occurrence ?? 1);
}

function queryRange(query, text) {
  const range = query.range ?? query;
  if (range.start && range.end) {
    return {
      start: positionFromLineCol(range.start.line, range.start.col),
      end: positionFromLineCol(range.end.line, range.end.col),
    };
  }
  if (range.startLine !== undefined || range.endLine !== undefined) {
    return {
      start: positionFromLineCol(range.startLine, range.startCol),
      end: positionFromLineCol(range.endLine, range.endCol),
    };
  }
  return {
    start: positionFromMarker(text, range.startMarker, range.startDelta ?? 0, range.startOccurrence ?? 1),
    end: positionFromMarker(text, range.endMarker, range.endDelta ?? 0, range.endOccurrence ?? 1),
  };
}

function singleQueryFromOptions(options) {
  if (!options.command) fail("missing query command; expected hover or inlay");
  const kind = options.command === "inlay-hints" ? "inlay" : options.command;
  if (!["hover", "inlay"].includes(kind)) fail(`unsupported query command: ${options.command}`);

  const query = { kind, label: options.label ?? kind };
  if (kind === "hover") {
    if (options.marker !== undefined) {
      query.marker = options.marker;
      query.delta = options.delta;
      query.occurrence = options.occurrence;
    } else {
      query.line = options.line;
      query.col = options.col;
    }
  } else {
    if (options.startMarker !== undefined || options.endMarker !== undefined) {
      query.range = {
        startMarker: options.startMarker,
        endMarker: options.endMarker,
      };
    } else {
      query.range = {
        startLine: options.startLine,
        startCol: options.startCol,
        endLine: options.endLine,
        endCol: options.endCol,
      };
    }
  }
  return query;
}

function loadQueryPlan(options, root) {
  const allowedConfigRoots = [root, fs.realpathSync(os.tmpdir())];
  if (options.queryFile) {
    const queryFile = resolveReadablePath(options.queryFile, allowedConfigRoots, "--query-file");
    const raw = readBoundedFile(queryFile, "--query-file");
    let plan;
    try {
      plan = JSON.parse(raw);
    } catch (error) {
      fail(`invalid query JSON: ${error.message}`);
    }
    return normalizePlan(plan, root, options);
  }

  if (!options.file) fail("--file is required without --query-file");
  return normalizePlan(
    {
      file: options.file,
      overlayFile: options.overlayFile,
      queries: [singleQueryFromOptions(options)],
    },
    root,
    options,
  );
}

function normalizePlan(plan, root, options) {
  if (!plan || typeof plan !== "object" || Array.isArray(plan)) fail("query plan must be an object");
  const file = resolveWorkspacePath(root, plan.file ?? options.file, "file");
  const allowedOverlayRoots = [root, fs.realpathSync(os.tmpdir())];
  const overlayFile = plan.overlayFile ?? options.overlayFile;
  const text = overlayFile
    ? readBoundedFile(resolveReadablePath(overlayFile, allowedOverlayRoots, "overlayFile"), "overlayFile")
    : readBoundedFile(file, "file");

  const queries = plan.queries;
  if (!Array.isArray(queries) || queries.length === 0) fail("query plan must contain queries");
  if (queries.length > MAX_QUERIES) fail(`too many queries: ${queries.length}, max ${MAX_QUERIES}`);

  for (const query of queries) {
    if (!query || typeof query !== "object" || Array.isArray(query)) fail("each query must be an object");
    if (!["hover", "inlay", "inlay-hints"].includes(query.kind)) {
      fail(`unsupported query kind: ${query.kind}`);
    }
    query.kind = query.kind === "inlay-hints" ? "inlay" : query.kind;
  }

  const format = plan.format ?? "text";
  if (!["text", "json"].includes(format)) fail("query plan format must be text or json");

  return { file, text, queries, format };
}

function releaseBinary(root, profile) {
  const binary = path.join(root, "target", profile, "rust-glancer");
  if (!fs.existsSync(binary)) {
    fail(
      `${binary} does not exist. Build it first with: cargo build --release -p rust-glancer`,
    );
  }
  return binary;
}

class LspClient {
  constructor(binary, root, timeoutMs, showLogs) {
    this.binary = binary;
    this.root = root;
    this.timeoutMs = timeoutMs;
    this.showLogs = showLogs;
    this.nextId = 1;
    this.pending = new Map();
    this.buffer = Buffer.alloc(0);
    this.stderr = "";
    this.child = spawn(binary, ["lsp"], {
      cwd: root,
      stdio: ["pipe", "pipe", "pipe"],
      shell: false,
    });
    this.child.stdout.on("data", (chunk) => this.onStdout(chunk));
    this.child.stderr.setEncoding("utf8");
    this.child.stderr.on("data", (chunk) => {
      this.stderr = (this.stderr + chunk).slice(-STDERR_TAIL_BYTES);
      if (this.showLogs) process.stderr.write(chunk);
    });
  }

  send(message) {
    const json = JSON.stringify(message);
    this.child.stdin.write(`Content-Length: ${Buffer.byteLength(json)}\r\n\r\n${json}`);
  }

  request(method, params) {
    const id = this.nextId;
    this.nextId += 1;
    this.send({ jsonrpc: "2.0", id, method, params });
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`timeout waiting for ${method}`));
      }, this.timeoutMs);
      this.pending.set(id, {
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        reject,
      });
    });
  }

  notify(method, params) {
    this.send({ jsonrpc: "2.0", method, params });
  }

  onStdout(chunk) {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    while (true) {
      const text = this.buffer.toString("utf8");
      const match = text.match(/^Content-Length: (\d+)\r\n\r\n/);
      if (!match) return;
      const length = Number(match[1]);
      const headerLength = Buffer.byteLength(match[0]);
      if (this.buffer.length < headerLength + length) return;
      const body = this.buffer.slice(headerLength, headerLength + length).toString("utf8");
      this.buffer = this.buffer.slice(headerLength + length);
      this.onMessage(JSON.parse(body));
    }
  }

  onMessage(message) {
    if (message.id !== undefined && this.pending.has(message.id)) {
      this.pending.get(message.id).resolve(message);
      this.pending.delete(message.id);
      return;
    }

    // The server may ask for progress/logging client capabilities during startup. Answering null
    // keeps the harness deterministic without needing a full client implementation.
    if (message.id !== undefined && message.method) {
      this.send({ jsonrpc: "2.0", id: message.id, result: null });
    }
  }

  async close() {
    try {
      await this.request("shutdown", null);
      this.notify("exit", null);
    } catch {
      // Fall through to process cleanup; query errors should not leave the LSP around.
    }
    if (!this.child.killed) {
      this.child.kill();
    }
  }
}

function hoverText(result) {
  if (!result) return null;
  const contents = result.contents;
  if (!contents) return null;
  if (typeof contents === "string") return contents;
  if (typeof contents.value === "string") return contents.value;
  if (Array.isArray(contents)) {
    return contents
      .map((item) => (typeof item === "string" ? item : item.value))
      .filter(Boolean)
      .join("\n");
  }
  return JSON.stringify(contents);
}

function normalizeHints(hints, maxHints) {
  return (hints ?? []).slice(0, maxHints).map((hint) => ({
    line: hint.position.line + 1,
    col: hint.position.character + 1,
    kind: hint.kind,
    label: typeof hint.label === "string" ? hint.label : JSON.stringify(hint.label),
  }));
}

async function run() {
  const options = parseArgs(process.argv.slice(2));
  const root = workspaceRoot();
  const plan = loadQueryPlan(options, root);
  const binary = releaseBinary(root, options.profile);
  const uri = pathToFileURL(plan.file).toString();
  const client = new LspClient(binary, root, options.timeoutMs, options.showLogs);
  const results = [];

  try {
    const initialize = await client.request("initialize", {
      processId: process.pid,
      rootUri: pathToFileURL(root).toString(),
      workspaceFolders: [{ uri: pathToFileURL(root).toString(), name: path.basename(root) }],
      capabilities: {
        textDocument: {
          hover: { contentFormat: ["markdown", "plaintext"] },
          inlayHint: { dynamicRegistration: false },
        },
      },
      initializationOptions: {
        cache: { packageResidency: options.packageResidency },
        diagnostics: { onStartup: false, onSave: false },
      },
    });
    if (initialize.error) fail(`initialize failed: ${JSON.stringify(initialize.error)}`);
    client.notify("initialized", {});
    client.notify("textDocument/didOpen", {
      textDocument: {
        uri,
        languageId: "rust",
        version: 1,
        text: plan.text,
      },
    });

    for (const query of plan.queries) {
      if (query.kind === "hover") {
        const position = queryPosition(query, plan.text);
        const response = await client.request("textDocument/hover", {
          textDocument: { uri },
          position,
        });
        results.push({
          kind: "hover",
          label: query.label ?? "hover",
          position: { line: position.line + 1, col: position.character + 1 },
          text: hoverText(response.result),
          raw: response.result,
          error: response.error,
        });
      } else if (query.kind === "inlay") {
        const range = queryRange(query, plan.text);
        const response = await client.request("textDocument/inlayHint", {
          textDocument: { uri },
          range,
        });
        results.push({
          kind: "inlay",
          label: query.label ?? "inlay",
          range: {
            start: { line: range.start.line + 1, col: range.start.character + 1 },
            end: { line: range.end.line + 1, col: range.end.character + 1 },
          },
          hints: normalizeHints(response.result, options.maxHints),
          truncated: (response.result ?? []).length > options.maxHints,
          raw: response.result,
          error: response.error,
        });
      }
    }
  } finally {
    await client.close();
  }

  const output = {
    file: path.relative(root, plan.file),
    binary: path.relative(root, binary),
    results,
    stderrTail: options.showLogs ? undefined : client.stderr,
  };

  if (options.json || plan.format === "json") {
    console.log(JSON.stringify(output, null, 2));
    return;
  }

  console.log(`file: ${output.file}`);
  console.log(`binary: ${output.binary}`);
  for (const result of results) {
    if (result.kind === "hover") {
      console.log(`\nhover ${result.label} @ ${result.position.line}:${result.position.col}`);
      console.log(result.text ?? "<no hover>");
    } else {
      console.log(`\ninlay ${result.label} @ ${result.range.start.line}:${result.range.start.col}..${result.range.end.line}:${result.range.end.col}`);
      for (const hint of result.hints) {
        console.log(`  ${hint.line}:${hint.col} ${hint.label}`);
      }
      if (result.truncated) console.log("  <truncated>");
    }
  }
}

run().catch((error) => {
  console.error(`lsp-query: ${error.stack ?? error.message}`);
  process.exit(1);
});
