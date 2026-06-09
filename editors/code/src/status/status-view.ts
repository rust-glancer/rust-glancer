/**
 * Renders rust-glancer's global status-bar item.
 *
 * This module knows how status states should look in VS Code: text, tooltip contents, background
 * color, command wiring, and plain snapshots for tests. It does not decide lifecycle state.
 */
import * as vscode from "vscode";

import { EXTENSION_COMMANDS } from "../commands";
import {
  statusText,
  type StatusDetails,
  type StatusSnapshot,
  type StatusState,
} from "./status-model";

export { statusText };
export type { StatusDetails, StatusSnapshot, StatusState } from "./status-model";

export class StatusView implements vscode.Disposable {
  private readonly item: vscode.StatusBarItem;
  private details: StatusDetails = {};
  private currentSnapshot: StatusSnapshot = {
    state: "created",
    text: "",
    details: {},
  };
  private disposed = false;

  public constructor() {
    this.item = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
    this.item.name = "Rust Glancer";
    this.item.command = EXTENSION_COMMANDS.restartServer;
  }

  public starting(details: StatusDetails): void {
    this.showState("starting", "$(sync~spin) Rust Glancer: starting", "Starting", details);
  }

  public indexing(details: StatusDetails = this.details): void {
    this.showState(
      "indexing",
      "$(sync~spin) Rust Glancer: indexing",
      "Indexing workspace",
      details,
    );
  }

  public ready(details: StatusDetails = this.details): void {
    this.showState("ready", "$(check) Rust Glancer: ready", "Ready", details);
  }

  public stale(details: StatusDetails = this.details): void {
    this.showState(
      "stale",
      "$(warning) Rust Glancer: stale until save",
      "Stale until save",
      details,
    );
  }

  public diagnosticsRunning(
    command: string | undefined,
    details: StatusDetails = this.details,
  ): void {
    this.showState(
      "diagnostics-running",
      "$(sync~spin) Rust Glancer: cargo check running",
      command === undefined ? "Cargo check running" : `Cargo check running: ${command}`,
      details,
    );
  }

  public diagnosticsFailed(details: StatusDetails = this.details): void {
    this.showState(
      "diagnostics-failed",
      "$(error) Rust Glancer: cargo check failed",
      "Cargo check failed",
      details,
      new vscode.ThemeColor("statusBarItem.errorBackground"),
    );
  }

  public stopped(reason: string, details: StatusDetails = this.details): void {
    this.showState(
      "stopped",
      "$(circle-slash) Rust Glancer: stopped",
      `Stopped: ${reason}`,
      details,
    );
  }

  public failed(reason: string, details: StatusDetails = this.details): void {
    this.showState(
      "failed",
      "$(error) Rust Glancer: failed",
      `Failed: ${reason}`,
      details,
      new vscode.ThemeColor("statusBarItem.errorBackground"),
    );
  }

  // Return plain data so tests do not need to inspect VS Code UI objects.
  public snapshot(): StatusSnapshot {
    return {
      ...this.currentSnapshot,
      details: { ...this.currentSnapshot.details },
    };
  }

  public dispose(): void {
    this.disposed = true;
    this.currentSnapshot = {
      state: "disposed",
      text: this.currentSnapshot.text,
      details: { ...this.currentSnapshot.details },
    };
    this.item.dispose();
  }

  private showState(
    state: StatusState,
    baseText: string,
    tooltipState: string,
    details: StatusDetails,
    backgroundColor: vscode.ThemeColor | undefined = undefined,
  ): void {
    if (this.disposed) {
      return;
    }

    this.details = details;
    this.currentSnapshot = {
      state,
      text: statusText(baseText, details),
      details: { ...details },
    };
    this.item.text = this.currentSnapshot.text;
    this.item.tooltip = this.tooltip(tooltipState);
    this.item.backgroundColor = backgroundColor;
    this.item.show();
  }

  private tooltip(state: string): vscode.MarkdownString {
    const tooltip = new vscode.MarkdownString();
    tooltip.appendMarkdown(`**Rust Glancer**\n\n`);
    appendTextField(tooltip, "State", state);

    if (this.details.activeWorkspaceRoot !== undefined) {
      appendCodeField(tooltip, "Active workspace", this.details.activeWorkspaceRoot);
    }
    if (this.details.workspaceRoot !== undefined) {
      appendCodeField(tooltip, "Server workspace", this.details.workspaceRoot);
    }
    if (this.details.serverCommand !== undefined) {
      appendCodeField(tooltip, "Server", this.details.serverCommand);
    }
    if (this.details.serverSource !== undefined) {
      appendTextField(tooltip, "Source", this.details.serverSource);
    }

    tooltip.appendMarkdown("Click to restart the server.");
    return tooltip;
  }
}

function appendTextField(tooltip: vscode.MarkdownString, label: string, value: string): void {
  tooltip.appendMarkdown(`${label}: `);
  tooltip.appendText(singleLine(value));
  tooltip.appendMarkdown("\n\n");
}

function appendCodeField(tooltip: vscode.MarkdownString, label: string, value: string): void {
  tooltip.appendMarkdown(`${label}: \``);
  tooltip.appendText(singleLine(value));
  tooltip.appendMarkdown("`\n\n");
}

function singleLine(value: string): string {
  return value.replace(/\s+/g, " ");
}
