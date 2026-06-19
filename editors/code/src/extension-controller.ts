/**
 * Controls the VS Code extension lifecycle.
 *
 * Multi-project routing belongs to the Rust LSP server. This controller wires VS Code events and
 * commands to the one window-level language-client slot.
 */
import * as vscode from "vscode";

import { LanguageClientSlot } from "./language-client/language-client-slot";
import type { LanguageClientSessionSnapshot } from "./language-client/language-client-session";
import { isRustFile } from "./utils/lsp-utils";
import { StatusView, type StatusSnapshot } from "./status/status-view";

export interface ExtensionControllerSnapshot {
  readonly status: StatusSnapshot;
  readonly session: LanguageClientSessionSnapshot | undefined;
}

export class ExtensionController implements vscode.Disposable {
  private readonly clientSlot: LanguageClientSlot;
  private readonly workspaceListeners: vscode.Disposable;

  /** Wires VS Code workspace/editor events to the window-level LSP client lifecycle. */
  public constructor(
    private readonly extensionLog: vscode.LogOutputChannel,
    serverOutput: vscode.OutputChannel,
    private readonly status: StatusView,
    extensionUri: vscode.Uri,
  ) {
    this.clientSlot = new LanguageClientSlot(extensionLog, serverOutput, status, extensionUri);
    this.workspaceListeners = vscode.Disposable.from(
      vscode.window.onDidChangeActiveTextEditor((editor) => {
        void this.activateForEditor(editor);
      }),
      vscode.workspace.onDidOpenTextDocument((document) => {
        void this.activateForDocument(document);
      }),
      vscode.workspace.onDidChangeTextDocument((event) => {
        this.refreshForDocument(event.document);
      }),
      vscode.workspace.onDidSaveTextDocument((document) => {
        this.refreshForDocument(document);
      }),
      vscode.workspace.onDidCloseTextDocument((document) => {
        this.refreshForDocument(document);
      }),
      vscode.workspace.onDidChangeWorkspaceFolders(() => {
        void this.handleWorkspaceFoldersChanged();
      }),
    );
  }

  /** Starts the window-level LSP session when the window has a filesystem workspace folder. */
  public async start(): Promise<void> {
    const workspaceFolder = this.selectedWorkspaceFolder();
    if (workspaceFolder === undefined) {
      this.extensionLog.info("no workspace folder found; rust-glancer server was not started");
      this.status.stopped("no workspace folder");
      return;
    }

    const client = await this.clientSlot.getSession(workspaceFolder);
    client?.refreshStatus();
  }

  /** Restarts the window-level LSP session, or starts it if it is not currently running. */
  public async restart(): Promise<void> {
    const workspaceFolder = this.selectedWorkspaceFolder();
    if (workspaceFolder === undefined) {
      void vscode.window.showWarningMessage("Rust Glancer needs an open workspace folder.");
      return;
    }

    const client = await this.clientSlot.replace(workspaceFolder);
    client?.refreshStatus();
  }

  /** Stops the window-level LSP session. Engine-specific stopping will live in the server later. */
  public async stopServer(): Promise<void> {
    const client = this.clientSlot.current();
    if (client === undefined || !client.isRunning()) {
      void vscode.window.showWarningMessage("Rust Glancer has no running server to stop.");
      return;
    }

    await this.clientSlot.stop();
  }

  /** Runs workspace reindexing against the active engine selected by the LSP server. */
  public async reindexWorkspace(): Promise<void> {
    const client = this.clientSlot.current();
    if (client === undefined || !client.isRunning()) {
      await this.start();
    }

    await this.clientSlot.current()?.reindexWorkspace();
  }

  /** Stops the managed language-client session and leaves the status view globally stopped. */
  public async stop(): Promise<void> {
    await this.clientSlot.stop();
    this.status.stopped("not running");
  }

  /** Captures controller state for extension tests and diagnostic commands. */
  public snapshot(): ExtensionControllerSnapshot {
    return {
      status: this.status.snapshot(),
      session: this.clientSlot.current()?.snapshot(),
    };
  }

  /** Releases VS Code listeners and asynchronously stops the running client, if any. */
  public dispose(): void {
    this.workspaceListeners.dispose();
    void this.stop();
  }

  private async activateForEditor(editor: vscode.TextEditor | undefined): Promise<boolean> {
    if (editor === undefined) {
      this.refreshActiveStatus();
      return false;
    }

    return this.activateForDocument(editor.document);
  }

  private async activateForDocument(document: vscode.TextDocument): Promise<boolean> {
    if (!isRustFile(document)) {
      this.refreshActiveStatus();
      return false;
    }

    const workspaceFolder = this.selectedWorkspaceFolder();
    if (workspaceFolder === undefined) {
      this.refreshActiveStatus();
      return false;
    }

    const client = await this.clientSlot.getSession(workspaceFolder);
    client?.refreshStatus();
    return client !== undefined;
  }

  private refreshForDocument(document: vscode.TextDocument): void {
    if (isRustFile(document)) {
      this.clientSlot.current()?.refreshStatus();
    }
    this.refreshActiveStatus();
  }

  private async handleWorkspaceFoldersChanged(): Promise<void> {
    const workspaceFolder = this.selectedWorkspaceFolder();
    if (workspaceFolder === undefined) {
      await this.stop();
      return;
    }

    // The current LSP protocol initialization contains the workspace folder list. Until the server
    // handles live workspace-folder notifications, restart so routing sees the new window shape.
    if (this.clientSlot.current()?.isRunning()) {
      const client = await this.clientSlot.replace(workspaceFolder);
      client?.refreshStatus();
    }
  }

  private selectedWorkspaceFolder(): vscode.WorkspaceFolder | undefined {
    const folders = (vscode.workspace.workspaceFolders ?? []).filter(
      (folder) => folder.uri.scheme === "file",
    );
    if (folders.length === 0) {
      return undefined;
    }

    const activeDocument = vscode.window.activeTextEditor?.document;
    if (activeDocument !== undefined && isRustFile(activeDocument)) {
      const activeFolder = vscode.workspace.getWorkspaceFolder(activeDocument.uri);
      if (activeFolder?.uri.scheme === "file") {
        return activeFolder;
      }
    }

    return folders[0];
  }

  private refreshActiveStatus(): void {
    const client = this.clientSlot.current();
    if (client !== undefined) {
      client.refreshStatus();
      return;
    }

    this.status.stopped("no active Rust workspace");
  }
}
