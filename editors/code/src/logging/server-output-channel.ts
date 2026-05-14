import * as vscode from "vscode";

import {
  type ParsedServerLogLine,
  formatRawServerLogLine,
  formatServerLogLine,
  parseServerLogLine,
} from "./server-log";

/// Adapts stderr text from `vscode-languageclient` into the server output channel.
///
/// The Rust side emits one structured JSON log event per line. Cargo startup output, panics, and
/// other non-JSON stderr still remain visible through the raw-line fallback.
export class ServerOutputChannel implements vscode.OutputChannel {
  private bufferedText = "";

  public constructor(private readonly inner: vscode.OutputChannel) {}

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
      this.inner.appendLine(formatServerLogLine(line.record));
    } else if (line.message.length > 0) {
      this.inner.appendLine(formatRawServerLogLine(line.level, line.message));
    }
  }
}
