/**
 * Test support for observing extension output without changing production behavior.
 *
 * Extension-host tests cannot read a normal VS Code output channel, so this wrapper records a
 * bounded text snapshot while still forwarding every call to the real UI channel.
 */
import * as vscode from "vscode";

export class RecordingOutputChannel implements vscode.OutputChannel {
  private readonly lines: string[] = [];

  public constructor(private readonly inner: vscode.OutputChannel) {}

  public get name(): string {
    return this.inner.name;
  }

  public append(value: string): void {
    this.record(value);
    this.inner.append(value);
  }

  public appendLine(value: string): void {
    this.record(`${value}\n`);
    this.inner.appendLine(value);
  }

  public replace(value: string): void {
    this.lines.length = 0;
    this.record(value);
    this.inner.replace(value);
  }

  public clear(): void {
    this.lines.length = 0;
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

  public snapshot(): string {
    return this.lines.join("");
  }

  private record(value: string): void {
    this.lines.push(value);
    if (this.lines.length > 1_000) {
      this.lines.splice(0, this.lines.length - 1_000);
    }
  }
}
