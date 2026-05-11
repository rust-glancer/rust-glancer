/**
 * Decides which language-client state should be rendered in the shared status view.
 *
 * The language client can be starting, indexing, stale, running diagnostics, failed, or ready based
 * on several independent event streams. This module merges those signals and delegates the final
 * rendering to `StatusView`.
 */
import {
  type ProgressToken,
  type WorkDoneProgressBegin,
  type WorkDoneProgressEnd,
  type WorkDoneProgressReport,
} from "vscode-languageclient/node";

import { StatusView, statusText, type StatusDetails, type StatusSnapshot } from "./status-view";

const CARGO_DIAGNOSTICS_PROGRESS_TITLE = "Cargo diagnostics";

export interface ClientStatusSnapshot {
  readonly running: boolean;
  readonly diagnosticsRunning: boolean;
  readonly diagnosticsFailed: boolean;
  readonly diagnosticsCommand: string | undefined;
  readonly failureReason: string | undefined;
  readonly status: StatusSnapshot;
  readonly details: StatusDetails | undefined;
}

/**
 * Tracks client-facing state and decides which status-bar state should win.
 *
 * VS Code document events, LSP lifecycle events, and work-done progress can arrive independently.
 * Keeping their merge logic here makes the extension controller mostly responsible for wiring.
 */
export class ClientStatus {
  private details: StatusDetails | undefined;
  private running = false;
  private diagnosticsRunning = false;
  private diagnosticsFailed = false;
  private diagnosticsCommand: string | undefined;
  private failureReason: string | undefined;
  private currentStatus: StatusSnapshot = {
    state: "created",
    text: "",
    details: {},
  };
  private readonly diagnosticsProgressTokens = new Set<ProgressToken>();

  public constructor(
    private readonly view: StatusView,
    private readonly shouldRender: () => boolean = () => true,
  ) {}

  public isRunning(): boolean {
    return this.running;
  }

  public currentDetails(): StatusDetails | undefined {
    return this.details === undefined ? undefined : { ...this.details };
  }

  public starting(details: StatusDetails): void {
    this.running = false;
    this.resetDiagnostics();
    this.failureReason = undefined;
    this.details = details;
    this.show("starting", "$(sync~spin) Rust Glancer: starting", () => this.view.starting(details));
  }

  public ready(details: StatusDetails): void {
    this.running = true;
    this.failureReason = undefined;
    this.details = details;
    this.show("ready", "$(check) Rust Glancer: ready", () => this.view.ready(details));
  }

  public indexing(): void {
    if (this.details === undefined) {
      return;
    }

    this.show("indexing", "$(sync~spin) Rust Glancer: indexing", () =>
      this.view.indexing(this.details),
    );
  }

  public activeWorkspace(root: string, isActiveRustDocumentDirty: boolean): void {
    if (this.details === undefined) {
      return;
    }

    this.details = {
      ...this.details,
      activeWorkspaceRoot: root,
    };
    this.refresh(isActiveRustDocumentDirty);
  }

  public stopped(reason: string, details: StatusDetails | undefined = this.details): void {
    this.running = false;
    this.resetDiagnostics();
    this.failureReason = undefined;
    this.details = details;
    this.show("stopped", "$(circle-slash) Rust Glancer: stopped", () =>
      this.view.stopped(reason, details ?? {}),
    );
  }

  public failed(reason: string, details: StatusDetails | undefined = this.details): void {
    this.running = false;
    this.resetDiagnostics();
    this.failureReason = reason;
    this.details = details;
    this.show("failed", "$(error) Rust Glancer: failed", () =>
      this.view.failed(reason, details ?? {}),
    );
  }

  public operationFailed(reason: string): void {
    if (this.details === undefined) {
      return;
    }

    // A failed request is user-visible, but it does not necessarily mean the LSP client stopped.
    this.failureReason = reason;
    this.show("failed", "$(error) Rust Glancer: failed", () =>
      this.view.failed(reason, this.details ?? {}),
    );
  }

  public refresh(isActiveRustDocumentDirty: boolean): void {
    if (!this.running || this.details === undefined) {
      return;
    }

    // Dirty buffers are shown first because the last published analysis no longer describes
    // the file the user is looking at. Cargo diagnostics remain visible once the editor is clean.
    if (isActiveRustDocumentDirty) {
      this.show("stale", "$(warning) Rust Glancer: stale until save", () =>
        this.view.stale(this.details),
      );
    } else if (this.diagnosticsRunning) {
      this.show("diagnostics-running", "$(sync~spin) Rust Glancer: cargo check running", () =>
        this.view.diagnosticsRunning(this.diagnosticsCommand, this.details),
      );
    } else if (this.diagnosticsFailed) {
      this.show("diagnostics-failed", "$(error) Rust Glancer: cargo check failed", () =>
        this.view.diagnosticsFailed(this.details),
      );
    } else {
      this.show("ready", "$(check) Rust Glancer: ready", () => this.view.ready(this.details));
    }
  }

  public handleWorkDoneProgress(
    token: ProgressToken,
    params: WorkDoneProgressBegin | WorkDoneProgressReport | WorkDoneProgressEnd,
    isActiveRustDocumentDirty: boolean,
  ): void {
    if (params.kind === "begin") {
      if (params.title !== CARGO_DIAGNOSTICS_PROGRESS_TITLE) {
        return;
      }

      this.diagnosticsProgressTokens.add(token);
      this.diagnosticsRunning = true;
      this.diagnosticsFailed = false;
      this.diagnosticsCommand = params.message;
      this.refresh(isActiveRustDocumentDirty);
      return;
    }

    if (!this.diagnosticsProgressTokens.has(token)) {
      return;
    }

    if (params.kind === "end") {
      this.diagnosticsProgressTokens.delete(token);
      this.diagnosticsRunning = this.diagnosticsProgressTokens.size > 0;
      this.diagnosticsFailed = params.message === "Failed";
      if (!this.diagnosticsRunning) {
        this.diagnosticsCommand = undefined;
      }
      this.refresh(isActiveRustDocumentDirty);
    }
  }

  public snapshot(): ClientStatusSnapshot {
    return {
      running: this.running,
      diagnosticsRunning: this.diagnosticsRunning,
      diagnosticsFailed: this.diagnosticsFailed,
      diagnosticsCommand: this.diagnosticsCommand,
      failureReason: this.failureReason,
      status: {
        ...this.currentStatus,
        details: { ...this.currentStatus.details },
      },
      details: this.currentDetails(),
    };
  }

  private show(state: StatusSnapshot["state"], baseText: string, render: () => void): void {
    this.currentStatus = {
      state,
      text: statusText(baseText, this.details),
      details: this.details === undefined ? {} : { ...this.details },
    };
    if (this.shouldRender()) {
      render();
    }
  }

  private resetDiagnostics(): void {
    this.diagnosticsRunning = false;
    this.diagnosticsFailed = false;
    this.diagnosticsCommand = undefined;
    this.diagnosticsProgressTokens.clear();
  }
}
