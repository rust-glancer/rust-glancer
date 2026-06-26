import * as vscode from "vscode";

import {
  type ParsedServerLogLine,
  formatRawServerLogLine,
  formatServerLogLine,
  parseServerLogLine,
} from "./server-log";
import { RecordingOutputChannel } from "../test-support/recording-output-channel";

const EXTENSION_TEST_ENV = "RUST_GLANCER_EXTENSION_TEST";
const SERVER_LOG_LANGUAGE_ID = "rust-glancer-log";
const SERVER_LOG_CHANNEL_NAME = "Rust Glancer Language Server";

export interface CreatedServerOutputChannel {
  readonly output: ServerOutputChannel;
  readonly recording?: RecordingOutputChannel;
}

export function createServerOutputChannel(): CreatedServerOutputChannel {
  const raw = vscode.window.createOutputChannel(SERVER_LOG_CHANNEL_NAME, SERVER_LOG_LANGUAGE_ID);
  const recording = isExtensionTestMode() ? new RecordingOutputChannel(raw) : undefined;

  return {
    output: new ServerOutputChannel(recording ?? raw),
    recording,
  };
}

export function isExtensionTestMode(): boolean {
  return process.env[EXTENSION_TEST_ENV] === "1";
}

/**
 * Defines the editor-facing log format for the Rust language server.
 *
 * We use a custom output language instead of VS Code's generic `log` grammar because the default
 * grammar is not flexible enough for compact structured logs. A normal line looks like this:
 *
 * `09:39:29.210 [d/rust-glancer/rg_lsp_engine::memory] memory report active=23.2MiB(+96.0KiB)`
 *
 * The prefix is `[level/source/target]`: `t/d/i/w/e` for trace/debug/info/warn/error, then the
 * server or engine name, then the Rust tracing target when one exists. Everything after the
 * message is `key=value` fields. The `rust-glancer-log` grammar colors those pieces and treats
 * each field value as one token, so paths, URIs, and memory values stay visually consistent.
 *
 * The Rust side still writes structured JSON lines to stderr. `server-log.ts` is the entrypoint
 * that parses those records, keeps raw stderr visible as a fallback, and formats both into this
 * compact display language.
 */
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
