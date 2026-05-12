import * as vscode from "vscode";

import {
  type ParsedServerLogLine,
  type ServerLogLevel,
  formatServerLogRecord,
  parseServerLogLine,
} from "./server-log";

/// Adapts stderr text from `vscode-languageclient` into VS Code's log-channel API.
///
/// The Rust side emits one structured JSON log event per line. Cargo startup output, panics, and
/// other non-JSON stderr still remain visible through the raw-line fallback.
export class ServerOutputChannel implements vscode.OutputChannel {
  private bufferedText = "";

  public constructor(private readonly inner: vscode.LogOutputChannel) {}

  public get name(): string {
    return this.inner.name;
  }

  public append(value: string): void {
    this.bufferedText += value;

    for (;;) {
      const newline = this.bufferedText.indexOf("\n");
      if (newline === -1) {
        return;
      }

      const line = this.bufferedText.slice(0, newline).replace(/\r$/, "");
      this.bufferedText = this.bufferedText.slice(newline + 1);
      this.publishLine(parseServerLogLine(line));
    }
  }

  public appendLine(value: string): void {
    this.append(`${value}\n`);
  }

  public replace(value: string): void {
    this.bufferedText = "";
    this.inner.clear();
    this.append(value);
  }

  public clear(): void {
    this.bufferedText = "";
    this.inner.clear();
  }

  public show(preserveFocus?: boolean): void;
  public show(column?: vscode.ViewColumn, preserveFocus?: boolean): void;
  public show(columnOrPreserveFocus?: vscode.ViewColumn | boolean, preserveFocus?: boolean): void {
    if (typeof columnOrPreserveFocus === "number") {
      this.inner.show(columnOrPreserveFocus, preserveFocus);
    } else {
      this.inner.show(columnOrPreserveFocus);
    }
  }

  public hide(): void {
    this.inner.hide();
  }

  public dispose(): void {
    this.inner.dispose();
  }

  private publishLine(line: ParsedServerLogLine): void {
    if (line.kind === "structured") {
      this.publish(line.record.level, formatServerLogRecord(line.record));
    } else if (line.message.length > 0) {
      this.publish(line.level, line.message);
    }
  }

  private publish(level: ServerLogLevel, message: string): void {
    switch (level) {
      case "trace":
        this.inner.trace(message);
        break;
      case "debug":
        this.inner.debug(message);
        break;
      case "warn":
        this.inner.warn(message);
        break;
      case "error":
        this.inner.error(message);
        break;
      case "info":
        this.inner.info(message);
        break;
    }
  }
}
