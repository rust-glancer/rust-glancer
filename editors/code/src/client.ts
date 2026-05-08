import * as vscode from "vscode";
import * as path from "node:path";

import { cargoWorkspaceManifest } from "./cargo-utils";
import { isFile, isPathInside, nearestCargoManifest } from "./fs-utils";
import { isRustFile } from "./lsp-utils";
import { WorkspaceClient, type WorkspaceClientSnapshot } from "./workspace_client";
import { StatusView, type StatusSnapshot } from "./status";

export interface ClientManagerSnapshot {
  readonly activeWorkspaceUri: string | undefined;
  readonly status: StatusSnapshot;
  readonly workspaces: WorkspaceClientSnapshot[];
}

/**
 * Coordinates one single-root rust-glancer client per Cargo workspace root.
 *
 * VS Code folders are often project catalogs rather than Cargo roots. The manager resolves the
 * Cargo project for the active Rust file, lazily starts a client for that root, routes commands to
 * the active project, and keeps the single status bar focused on the visible root.
 */
export class ClientManager implements vscode.Disposable {
  private readonly clients = new Map<string, WorkspaceClient>();
  private readonly cargoRootByManifest = new Map<string, Promise<vscode.WorkspaceFolder>>();
  private readonly workspaceListeners: vscode.Disposable;

  public constructor(
    private readonly extensionPath: string,
    private readonly output: vscode.OutputChannel,
    private readonly status: StatusView,
  ) {
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

  public async start(): Promise<void> {
    if (await this.activateWorkspaceForEditor(vscode.window.activeTextEditor)) {
      return;
    }

    const cargoFolders = await this.cargoWorkspaceFolders();
    if (cargoFolders.length === 1) {
      await this.ensureClientForFolder(cargoFolders[0]);
      this.refreshActiveStatus();
      return;
    }

    if (cargoFolders.length === 0) {
      this.output.appendLine("no Cargo workspace folder found; rust-glancer server was not started");
      this.status.stopped("no Cargo workspace folder");
    } else {
      this.output.appendLine(
        "multiple Cargo workspace folders detected; rust-glancer will start when a Rust file is opened",
      );
      this.status.stopped("select a Rust workspace");
    }
  }

  public async restart(): Promise<void> {
    const client = await this.commandTargetClient("restart");
    if (client === undefined) {
      return;
    }

    await client.restart();
    this.refreshActiveStatus();
  }

  public async stopServer(): Promise<void> {
    const client = await this.stopTargetClient();
    if (client === undefined) {
      return;
    }

    this.clients.delete(client.workspaceKey());
    await client.stop();
    this.refreshActiveStatus();
  }

  public async reindexWorkspace(): Promise<void> {
    const client = await this.commandTargetClient("reindex");
    if (client === undefined) {
      return;
    }

    await client.reindexWorkspace();
    this.refreshActiveStatus();
  }

  public async stop(): Promise<void> {
    const clients = [...this.clients.values()];
    this.clients.clear();

    await Promise.all(clients.map((client) => client.stop()));
    this.status.stopped("not running");
  }

  public snapshot(): ClientManagerSnapshot {
    return {
      activeWorkspaceUri: this.activeWorkspaceKey(),
      status: this.status.snapshot(),
      workspaces: [...this.clients.values()].map((client) => client.snapshot()),
    };
  }

  public dispose(): void {
    this.workspaceListeners.dispose();
    void this.stop();
  }

  private async activateWorkspaceForEditor(
    editor: vscode.TextEditor | undefined,
  ): Promise<boolean> {
    if (editor === undefined) {
      this.refreshActiveStatus();
      return false;
    }

    return this.activateWorkspaceForDocument(editor.document);
  }

  private async activateWorkspaceForDocument(document: vscode.TextDocument): Promise<boolean> {
    if (!isRustFile(document)) {
      this.refreshActiveStatus();
      return false;
    }

    const workspaceFolder = await this.cargoRootForDocument(document);
    if (workspaceFolder === undefined) {
      this.refreshActiveStatus();
      return false;
    }

    const client = await this.ensureClientForFolder(workspaceFolder);
    client?.refreshStatus();
    return client !== undefined;
  }

  private async ensureClientForFolder(
    workspaceFolder: vscode.WorkspaceFolder,
  ): Promise<WorkspaceClient | undefined> {
    if (!(await isFile(path.join(workspaceFolder.uri.fsPath, "Cargo.toml")))) {
      return undefined;
    }

    const key = workspaceFolder.uri.toString();
    const existing = this.clients.get(key);
    if (existing !== undefined) {
      return existing;
    }

    const client = new WorkspaceClient(
      this.extensionPath,
      this.output,
      this.status,
      workspaceFolder,
      () => this.visibleWorkspaceKey() === key,
    );
    this.clients.set(key, client);
    await client.start();
    return client;
  }

  private refreshWorkspaceForDocument(document: vscode.TextDocument): void {
    this.clientForDocument(document)?.refreshStatus();
    this.refreshActiveStatus();
  }

  private async handleWorkspaceFoldersChanged(
    event: vscode.WorkspaceFoldersChangeEvent,
  ): Promise<void> {
    for (const folder of event.removed) {
      for (const [key, client] of [...this.clients.entries()]) {
        if (!isPathInside(client.workspaceRoot(), folder.uri.fsPath)) {
          continue;
        }

        this.clients.delete(key);
        await client.stop();
      }
    }

    await this.activateWorkspaceForEditor(vscode.window.activeTextEditor);
    this.refreshActiveStatus();
  }

  private async commandTargetClient(action: string): Promise<WorkspaceClient | undefined> {
    // Commands should follow the editor focus when possible. If that workspace has not been
    // started yet, starting it here is less surprising than acting on some other running client.
    const activeWorkspace = await this.editorCargoRoot();
    if (activeWorkspace !== undefined) {
      const active = await this.ensureClientForFolder(activeWorkspace);
      if (active !== undefined) {
        return active;
      }
    }

    if (activeWorkspace === undefined && this.clients.size === 0) {
      await this.start();
      const started = this.activeClient();
      if (started !== undefined) {
        return started;
      }
    }

    if (activeWorkspace === undefined && this.clients.size === 1) {
      return [...this.clients.values()][0];
    }

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

  private async stopTargetClient(): Promise<WorkspaceClient | undefined> {
    const activeDocument = vscode.window.activeTextEditor?.document;
    if (activeDocument !== undefined) {
      const activeClient = this.clientForDocument(activeDocument);
      if (activeClient !== undefined) {
        return activeClient;
      }

      if (await this.cargoRootForDocument(activeDocument)) {
        void vscode.window.showWarningMessage("Rust Glancer is not running for this workspace.");
        return undefined;
      }
    }

    const runningClients = [...this.clients.values()].filter((client) => client.isRunning());
    if (runningClients.length === 0) {
      void vscode.window.showWarningMessage("Rust Glancer has no running servers to stop.");
      return undefined;
    }

    if (runningClients.length === 1) {
      return runningClients[0];
    }

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

  private activeClient(): WorkspaceClient | undefined {
    const activeKey = this.visibleWorkspaceKey();
    if (activeKey !== undefined) {
      return this.clients.get(activeKey);
    }
    return undefined;
  }

  private activeWorkspaceKey(): string | undefined {
    const document = vscode.window.activeTextEditor?.document;
    if (document === undefined) {
      return undefined;
    }

    return this.clientForDocument(document)?.workspaceKey();
  }

  private visibleWorkspaceKey(): string | undefined {
    const document = vscode.window.activeTextEditor?.document;
    if (document !== undefined && isRustFile(document)) {
      return this.clientForDocument(document)?.workspaceKey();
    }

    const activeKey = this.activeWorkspaceKey();
    if (activeKey !== undefined) {
      return activeKey;
    }

    if (this.clients.size === 1) {
      return [...this.clients.keys()][0];
    }

    return undefined;
  }

  private async editorCargoRoot(): Promise<vscode.WorkspaceFolder | undefined> {
    const document = vscode.window.activeTextEditor?.document;
    return document === undefined ? undefined : this.cargoRootForDocument(document);
  }

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

  private async cargoWorkspaceFolders(): Promise<vscode.WorkspaceFolder[]> {
    const folders = vscode.workspace.workspaceFolders ?? [];
    const cargoFolders = new Map<string, vscode.WorkspaceFolder>();
    for (const folder of folders) {
      const cargoRoot = await this.cargoRootForDirectory(folder.uri, {
        containingFolder: folder,
      });
      if (cargoRoot !== undefined) {
        cargoFolders.set(cargoRoot.uri.toString(), cargoRoot);
      }
    }
    return [...cargoFolders.values()];
  }

  private clientForDocument(document: vscode.TextDocument): WorkspaceClient | undefined {
    if (!isRustFile(document)) {
      return undefined;
    }

    return [...this.clients.values()].find((client) => client.containsDocument(document));
  }

  private async cargoRootForDocument(
    document: vscode.TextDocument,
  ): Promise<vscode.WorkspaceFolder | undefined> {
    if (!isRustFile(document)) {
      return undefined;
    }

    const containingFolder = vscode.workspace.getWorkspaceFolder(document.uri);
    return this.cargoRootForDirectory(vscode.Uri.file(path.dirname(document.uri.fsPath)), {
      containingFolder,
    });
  }

  private async cargoRootForDirectory(
    directory: vscode.Uri,
    options: { containingFolder?: vscode.WorkspaceFolder } = {},
  ): Promise<vscode.WorkspaceFolder | undefined> {
    if (directory.scheme !== "file") {
      return undefined;
    }

    const manifest = await nearestCargoManifest(
      directory.fsPath,
      options.containingFolder?.uri.fsPath,
    );
    if (manifest === undefined) {
      return undefined;
    }

    return this.cargoRootForManifest(manifest, options.containingFolder);
  }

  private async cargoRootForManifest(
    manifestPath: string,
    containingFolder: vscode.WorkspaceFolder | undefined,
  ): Promise<vscode.WorkspaceFolder> {
    const existing = this.cargoRootByManifest.get(manifestPath);
    if (existing !== undefined) {
      return existing;
    }

    const resolved = this.resolveCargoRootForManifest(manifestPath, containingFolder);
    this.cargoRootByManifest.set(manifestPath, resolved);
    return resolved;
  }

  private async resolveCargoRootForManifest(
    manifestPath: string,
    containingFolder: vscode.WorkspaceFolder | undefined,
  ): Promise<vscode.WorkspaceFolder> {
    let rootManifest = manifestPath;
    try {
      rootManifest = await cargoWorkspaceManifest(manifestPath);
    } catch (error) {
      // Root discovery is an extension convenience, not the source of truth. If Cargo is missing or
      // `locate-project` fails for a partially edited manifest, the server can still try the nearest
      // manifest and report the real initialization error if that is not enough.
      this.output.appendLine(
        `cargo workspace root discovery failed for ${manifestPath}: ${String(error)}`,
      );
    }

    const root = path.dirname(rootManifest);
    return {
      uri: vscode.Uri.file(root),
      name: path.basename(root),
      index: containingFolder?.index ?? 0,
    };
  }
}
