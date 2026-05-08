/**
 * Orchestrates all rust-glancer clients owned by the VS Code extension.
 *
 * The server itself is single-root, so this module maps editor/workspace events onto one
 * `WorkspaceClient` per Cargo root and coordinates global commands such as activation, restart,
 * stopping, reindexing, and status selection.
 */
import * as vscode from "vscode";
import * as path from "node:path";

import { isFile } from "./utils/fs-utils";
import { isRustFile } from "./utils/lsp-utils";
import { WorkspaceClient, type WorkspaceClientSnapshot } from "./workspace/workspace-client";
import { StatusView, type StatusSnapshot } from "./status/status-view";
import { planRestartTarget } from "./workspace/workspace-lifecycle";
import {
  CargoWorkspaceResolver,
  type ResolvedCargoWorkspace,
} from "./workspace/workspace-resolution";

export interface ClientManagerSnapshot {
  readonly activeWorkspaceUri: string | undefined;
  readonly status: StatusSnapshot;
  readonly workspaces: WorkspaceClientSnapshot[];
}

export class ClientManager implements vscode.Disposable {
  private readonly clients = new Map<string, WorkspaceClient>();
  private readonly workspaceResolver: CargoWorkspaceResolver;
  private readonly workspaceListeners: vscode.Disposable;

  /** Wires VS Code workspace/editor events to the client lifecycle manager. */
  public constructor(
    private readonly extensionPath: string,
    private readonly output: vscode.OutputChannel,
    private readonly status: StatusView,
  ) {
    this.workspaceResolver = new CargoWorkspaceResolver(output);
    this.workspaceListeners = vscode.Disposable.from(
      vscode.window.onDidChangeActiveTextEditor((editor) => {
        void this.activateWorkspaceForEditor(editor);
      }),
      vscode.workspace.onDidOpenTextDocument((document) => {
        void this.activateWorkspaceForDocument(document);
      }),
      vscode.workspace.onDidChangeTextDocument((event) => {
        this.refreshWorkspaceForDocument(event.document);
      }),
      vscode.workspace.onDidSaveTextDocument((document) => {
        this.refreshWorkspaceForDocument(document);
      }),
      vscode.workspace.onDidCloseTextDocument((document) => {
        this.refreshWorkspaceForDocument(document);
      }),
      vscode.workspace.onDidChangeWorkspaceFolders((event) => {
        void this.handleWorkspaceFoldersChanged(event);
      }),
    );
  }

  /** Starts rust-glancer for the active Rust workspace, or for the only discovered workspace. */
  public async start(): Promise<void> {
    // The active editor is the best signal for which Cargo workspace the user expects to run.
    if (await this.activateWorkspaceForEditor(vscode.window.activeTextEditor)) {
      return;
    }

    // If every opened folder resolves to the same Cargo root, start that shared client eagerly.
    const cargoFolders = await this.workspaceResolver.workspaceFolders();
    const uniqueCargoRoots = new Set(cargoFolders.map((workspace) => workspace.workspaceKey));
    if (uniqueCargoRoots.size === 1) {
      for (const workspace of cargoFolders) {
        await this.ensureClientForWorkspace(workspace);
      }
      this.refreshActiveStatus();
      return;
    }

    // Ambiguous windows wait for a Rust file to become active before starting a server.
    if (cargoFolders.length === 0) {
      this.output.appendLine(
        "no Cargo workspace folder found; rust-glancer server was not started",
      );
      this.status.stopped("no Cargo workspace folder");
    } else {
      this.output.appendLine(
        "multiple Cargo workspace folders detected; rust-glancer will start when a Rust file is opened",
      );
      this.status.stopped("select a Rust workspace");
    }
  }

  /** Restarts the selected running client, or starts the active workspace if it was stopped. */
  public async restart(): Promise<void> {
    const target = await this.restartTargetClient();
    if (target === undefined) {
      return;
    }

    // A stopped active workspace is already started by target resolution, so do not restart it.
    if (target.restart) {
      await target.client.restart();
    } else {
      target.client.refreshStatus();
    }
    this.refreshActiveStatus();
  }

  /** Stops the client chosen for the current editor, single workspace, or user selection. */
  public async stopServer(): Promise<void> {
    const client = await this.stopTargetClient();
    if (client === undefined) {
      return;
    }

    this.clients.delete(client.workspaceKey());
    await client.stop();
    this.refreshActiveStatus();
  }

  /** Runs workspace reindexing against the command target selected from editor context. */
  public async reindexWorkspace(): Promise<void> {
    const client = await this.commandTargetClient("reindex");
    if (client === undefined) {
      return;
    }

    await client.reindexWorkspace();
    this.refreshActiveStatus();
  }

  /** Stops every managed language client and leaves the status view in a global stopped state. */
  public async stop(): Promise<void> {
    const clients = [...this.clients.values()];
    this.clients.clear();

    await Promise.all(clients.map((client) => client.stop()));
    this.status.stopped("not running");
  }

  /** Captures the manager state for extension tests and diagnostic commands. */
  public snapshot(): ClientManagerSnapshot {
    return {
      activeWorkspaceUri: this.activeWorkspaceKey(),
      status: this.status.snapshot(),
      workspaces: [...this.clients.values()].map((client) => client.snapshot()),
    };
  }

  /** Releases VS Code listeners and asynchronously stops any running clients. */
  public dispose(): void {
    this.workspaceListeners.dispose();
    void this.stop();
  }

  /**
   * Activates the workspace for an editor.
   *
   * Returns true when a rust-glancer client was found or started for that editor; false means no
   * Rust workspace was activated and callers should use their fallback behavior.
   */
  private async activateWorkspaceForEditor(
    editor: vscode.TextEditor | undefined,
  ): Promise<boolean> {
    // With no editor, there is no active document to route by.
    if (editor === undefined) {
      this.refreshActiveStatus();
      return false;
    }

    return this.activateWorkspaceForDocument(editor.document);
  }

  /**
   * Activates the workspace that owns a document.
   *
   * Returns true when a client is available for the document's Cargo workspace; false means the
   * document should not drive rust-glancer activation.
   */
  private async activateWorkspaceForDocument(document: vscode.TextDocument): Promise<boolean> {
    // We only activate the LSP server from Rust documents.
    if (!isRustFile(document)) {
      this.refreshActiveStatus();
      return false;
    }

    // Resolve the Cargo root, owner folder, and settings resource as one workspace identity.
    const workspace = await this.workspaceResolver.forDocument(document);
    if (workspace === undefined) {
      this.refreshActiveStatus();
      return false;
    }

    // Reuse an existing client for that Cargo root, or start a new one if needed.
    const client = await this.ensureClientForWorkspace(workspace);
    client?.refreshStatus();
    return client !== undefined;
  }

  /** Ensures one language client exists for a resolved Cargo workspace root. */
  private async ensureClientForWorkspace(
    workspace: ResolvedCargoWorkspace,
  ): Promise<WorkspaceClient | undefined> {
    const workspaceFolder = workspace.cargoFolder;
    // The resolver can fall back to a manifest path; verify the root still has a real manifest.
    if (!(await isFile(path.join(workspaceFolder.uri.fsPath, "Cargo.toml")))) {
      return undefined;
    }

    const key = workspace.workspaceKey;
    const existing = this.clients.get(key);
    if (existing !== undefined) {
      // A shared Cargo-root client may gain another opened member folder as an owner.
      existing.addOwner(workspace.ownerKey);
      existing.useConfigResource(workspace.configResource);
      return existing;
    }

    // Create the client lazily so multi-root windows only pay for workspaces they actually use.
    const client = new WorkspaceClient(
      this.extensionPath,
      this.output,
      this.status,
      workspaceFolder,
      workspace.configResource,
      workspace.ownerKey,
      () => this.visibleWorkspaceKey() === key,
    );
    this.clients.set(key, client);
    await client.start();
    return client;
  }

  /** Refreshes any running client affected by a document edit/save/close event. */
  private refreshWorkspaceForDocument(document: vscode.TextDocument): void {
    this.clientForDocument(document)?.refreshStatus();
    this.refreshActiveStatus();
  }

  /** Removes deleted VS Code folders as client owners and stops clients with no owners left. */
  private async handleWorkspaceFoldersChanged(
    event: vscode.WorkspaceFoldersChangeEvent,
  ): Promise<void> {
    // Folder changes can alter nearest-manifest boundaries and Cargo root resolution.
    this.workspaceResolver.clearCache();

    for (const folder of event.removed) {
      for (const [key, client] of [...this.clients.entries()]) {
        // Removing one member folder should not stop a parent-root client still owned elsewhere.
        client.removeOwner(folder.uri.toString());
        if (!client.hasNoOwners()) {
          continue;
        }

        this.clients.delete(key);
        await client.stop();
      }
    }

    // The active editor may now belong to a newly added folder or to a client that was stopped.
    await this.activateWorkspaceForEditor(vscode.window.activeTextEditor);
    this.refreshActiveStatus();
  }

  /**
   * Resolves the command target for Restart Server.
   *
   * The returned flag tells the caller whether to run a real stop/start restart. A false flag means
   * target selection already started the client and the caller should only refresh status.
   */
  private async restartTargetClient(): Promise<
    { client: WorkspaceClient; restart: boolean } | undefined
  > {
    // The active Rust document has priority, but only already-running clients should restart.
    const activeWorkspace = await this.editorCargoRoot();
    const activeWorkspaceKey = activeWorkspace?.workspaceKey;
    const plan = planRestartTarget({
      activeWorkspaceKey,
      existingWorkspaceKeys: [...this.clients.keys()],
    });

    switch (plan.kind) {
      case "restart-existing": {
        // Preserve the active folder as an owner before restarting its shared Cargo-root client.
        const client = this.clients.get(plan.workspaceKey);
        if (client !== undefined && activeWorkspace !== undefined) {
          client.addOwner(activeWorkspace.ownerKey);
          client.useConfigResource(activeWorkspace.configResource);
          return { client, restart: true };
        }
        return client === undefined ? undefined : { client, restart: true };
      }
      case "start-active": {
        // Starting a stopped active workspace is the requested outcome; restarting would double it.
        if (activeWorkspace === undefined) {
          return undefined;
        }
        const client = await this.ensureClientForWorkspace(activeWorkspace);
        return client === undefined ? undefined : { client, restart: false };
      }
      case "start-discovered": {
        // With no active editor and no clients, reuse startup discovery as the fallback target.
        await this.start();
        const client = this.activeClient();
        return client === undefined ? undefined : { client, restart: false };
      }
      case "restart-single": {
        // A single existing client is unambiguous even without an active Rust editor.
        const client = this.clients.get(plan.workspaceKey);
        return client === undefined ? undefined : { client, restart: true };
      }
      case "prompt":
        // Multiple running clients need the user to choose which workspace command should affect.
        return this.pickClient("restart").then((client) =>
          client === undefined ? undefined : { client, restart: true },
        );
    }
  }

  /** Chooses a client for commands that can start an inactive active workspace. */
  private async commandTargetClient(action: string): Promise<WorkspaceClient | undefined> {
    // Commands should follow the editor focus when possible. If that workspace has not been
    // started yet, starting it here is less surprising than acting on some other running client.
    const activeWorkspace = await this.editorCargoRoot();
    if (activeWorkspace !== undefined) {
      const active = await this.ensureClientForWorkspace(activeWorkspace);
      if (active !== undefined) {
        return active;
      }
    }

    // With no active Rust editor, startup discovery may still find one obvious workspace.
    if (activeWorkspace === undefined && this.clients.size === 0) {
      await this.start();
      const started = this.activeClient();
      if (started !== undefined) {
        return started;
      }
    }

    // A single running client is a clear command target when the editor gives no Rust context.
    if (activeWorkspace === undefined && this.clients.size === 1) {
      return [...this.clients.values()][0];
    }

    return this.pickClient(action);
  }

  /** Prompts the user to choose one managed client for a command. */
  private async pickClient(action: string): Promise<WorkspaceClient | undefined> {
    const choices = [...this.clients.values()].map((client) => ({
      label: client.workspaceName(),
      description: client.workspaceRoot(),
      client,
    }));
    const picked = await vscode.window.showQuickPick(choices, {
      placeHolder: `Select workspace to ${action}`,
    });
    return picked?.client;
  }

  /** Chooses the safest target for Stop Server without starting any new clients. */
  private async stopTargetClient(): Promise<WorkspaceClient | undefined> {
    const activeDocument = vscode.window.activeTextEditor?.document;
    if (activeDocument !== undefined) {
      // Stop follows the active document only when that document already has a running client.
      const activeClient = this.clientForDocument(activeDocument);
      if (activeClient !== undefined) {
        return activeClient;
      }

      // A Rust workspace with no running client should report that fact instead of stopping another.
      if (await this.workspaceResolver.forDocument(activeDocument)) {
        void vscode.window.showWarningMessage("Rust Glancer is not running for this workspace.");
        return undefined;
      }
    }

    // Without an active Rust target, only currently running clients are valid stop candidates.
    const runningClients = [...this.clients.values()].filter((client) => client.isRunning());
    if (runningClients.length === 0) {
      void vscode.window.showWarningMessage("Rust Glancer has no running servers to stop.");
      return undefined;
    }

    if (runningClients.length === 1) {
      return runningClients[0];
    }

    // Multiple running clients need an explicit choice to avoid stopping the wrong workspace.
    const choices = runningClients.map((client) => ({
      label: client.workspaceName(),
      description: client.workspaceRoot(),
      client,
    }));
    const picked = await vscode.window.showQuickPick(choices, {
      placeHolder: "Select workspace server to stop",
    });
    return picked?.client;
  }

  /** Returns the client that should drive status for the currently visible workspace. */
  private activeClient(): WorkspaceClient | undefined {
    const activeKey = this.visibleWorkspaceKey();
    if (activeKey !== undefined) {
      return this.clients.get(activeKey);
    }
    return undefined;
  }

  /** Returns the running workspace key for the active document, if rust-glancer owns it. */
  private activeWorkspaceKey(): string | undefined {
    const document = vscode.window.activeTextEditor?.document;
    if (document === undefined) {
      return undefined;
    }

    return this.clientForDocument(document)?.workspaceKey();
  }

  /** Chooses the workspace key whose status should be shown in the single status item. */
  private visibleWorkspaceKey(): string | undefined {
    // Rust editors only show status if the document belongs to a running client.
    const document = vscode.window.activeTextEditor?.document;
    if (document !== undefined && isRustFile(document)) {
      return this.clientForDocument(document)?.workspaceKey();
    }

    // Reuse the active-document helper before falling back to the single-client case.
    const activeKey = this.activeWorkspaceKey();
    if (activeKey !== undefined) {
      return activeKey;
    }

    // A single client is unambiguous enough to show even without editor context.
    if (this.clients.size === 1) {
      return [...this.clients.keys()][0];
    }

    return undefined;
  }

  /** Resolves the active editor's Cargo workspace without starting a client. */
  private async editorCargoRoot(): Promise<ResolvedCargoWorkspace | undefined> {
    const document = vscode.window.activeTextEditor?.document;
    return document === undefined ? undefined : this.workspaceResolver.forDocument(document);
  }

  /** Updates the status view from the active client, or explains why no client is active. */
  private refreshActiveStatus(): void {
    const active = this.activeClient();
    if (active !== undefined) {
      active.refreshStatus();
      return;
    }

    if (this.clients.size === 0) {
      this.status.stopped("no active Rust workspace");
    } else {
      this.status.stopped("select a Rust workspace");
    }
  }

  /** Finds the running client whose Cargo root contains a Rust document. */
  private clientForDocument(document: vscode.TextDocument): WorkspaceClient | undefined {
    if (!isRustFile(document)) {
      return undefined;
    }

    return [...this.clients.values()].find((client) => client.containsDocument(document));
  }
}
