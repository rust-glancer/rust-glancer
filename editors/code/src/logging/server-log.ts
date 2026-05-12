/**
 * Parses structured logs emitted by the Rust LSP server and engine subprocesses.
 *
 * In LSP mode, Rust writes one JSON object per stderr line. Each event carries a schema marker,
 * severity level, human message, component/engine identity, and a small bag of structured fields.
 * The extension keeps this parser narrow so unknown or non-JSON stderr remains visible as raw log
 * output instead of disappearing.
 */
const SERVER_LOG_SCHEMA = "rust-glancer-log/v1";

export type ServerLogLevel = "trace" | "debug" | "info" | "warn" | "error";

export interface ServerLogRecord {
  readonly level: ServerLogLevel;
  readonly component: string;
  readonly engine?: string;
  readonly message: string;
  readonly target?: string;
  readonly fields: Record<string, unknown>;
}

export type ParsedServerLogLine =
  | {
      readonly kind: "structured";
      readonly record: ServerLogRecord;
    }
  | {
      readonly kind: "raw";
      readonly level: ServerLogLevel;
      readonly message: string;
    };

export function parseServerLogLine(line: string): ParsedServerLogLine {
  const trimmed = line.trim();
  if (trimmed.length === 0) {
    return { kind: "raw", level: "info", message: "" };
  }

  const parsed = parseJsonObject(trimmed);
  if (parsed === undefined || parsed.schema !== SERVER_LOG_SCHEMA) {
    return { kind: "raw", level: rawLineLevel(trimmed), message: trimmed };
  }

  const level = logLevel(parsed.level);
  const message = stringValue(parsed.message) ?? "";
  const component = stringValue(parsed.component) ?? "server";
  const engine = stringValue(parsed.engine);
  const target = stringValue(parsed.target);
  const fields = recordValue(parsed.fields) ?? {};

  return {
    kind: "structured",
    record: {
      level,
      component,
      engine,
      message,
      target,
      fields,
    },
  };
}

export function formatServerLogRecord(record: ServerLogRecord): string {
  const source =
    record.component === "engine" && record.engine !== undefined
      ? `${record.component}:${record.engine}`
      : record.component;
  const fields = formatFields(record.fields);

  return fields.length === 0
    ? `[${source}] ${record.message}`
    : `[${source}] ${record.message} ${fields}`;
}

function parseJsonObject(line: string): Record<string, unknown> | undefined {
  try {
    const parsed = JSON.parse(line) as unknown;
    return recordValue(parsed);
  } catch (_error) {
    return undefined;
  }
}

function logLevel(value: unknown): ServerLogLevel {
  switch (stringValue(value)?.toLowerCase()) {
    case "trace":
      return "trace";
    case "debug":
      return "debug";
    case "warn":
    case "warning":
      return "warn";
    case "error":
      return "error";
    default:
      return "info";
  }
}

function rawLineLevel(line: string): ServerLogLevel {
  return /(^|\b)(error|panic|fatal)(\b|:)/i.test(line) ? "error" : "info";
}

function stringValue(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function recordValue(value: unknown): Record<string, unknown> | undefined {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return undefined;
  }

  return value as Record<string, unknown>;
}

function formatFields(fields: Record<string, unknown>): string {
  return Object.entries(fields)
    .map(([key, value]) => `${key}=${formatFieldValue(value)}`)
    .join(" ");
}

function formatFieldValue(value: unknown): string {
  if (typeof value === "string") {
    return quoteIfNeeded(value);
  }
  if (typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }
  if (value === null) {
    return "null";
  }

  return JSON.stringify(value);
}

function quoteIfNeeded(value: string): string {
  return /^[A-Za-z0-9_./:-]+$/.test(value) ? value : JSON.stringify(value);
}
