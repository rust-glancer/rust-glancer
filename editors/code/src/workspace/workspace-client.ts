/**
 * Owns one VS Code language-client instance for one resolved Cargo workspace root.
 *
 * This module is the boundary between extension orchestration and the LSP client process: it starts
 * and stops the server, forwards reindex requests, wires middleware, and maintains per-client
 * status state.
 */
import * as vscode from "vscode";
import * as path from "node:path";
import {
  ExecuteCommandRequest,
  LanguageClient,
  State,
  type LanguageClientOptions,
  Trace,
} from "vscode-languageclient/node";

import { SERVER_COMMANDS } from "../commands";
import { ExtensionConfig, type TraceSetting } from "../config";
import { ClientStatus, type ClientStatusSnapshot } from "../status/client-status";
import { hoverMiddleware } from "../features/hover-actions";
import { isRustFile } from "../utils/lsp-utils";
import { ResolvedServer } from "./server";
import { StatusView } from "../status/status-view";
import { WorkspaceOwners } from "./workspace-lifecycle";

export interface WorkspaceClientSnapshot extends ClientStatusSnapshot {
  readonly workspaceRoot: string;
  readonly workspaceUri: string;
  readonly ownerKeys: string[];
  readonly hasClient: boolean;
}

export class WorkspaceClient implements vscode.Disposable {
  private client: LanguageClient | undefined;
  private clientState: vscode.Disposable | undefined;
  private readonly clientStatus: ClientStatus;
  private readonly owners: WorkspaceOwners;

  public constructor(
    private readonly extensionPath: string,
    private readonly output: vscode.OutputChannel,
    private readonly status: StatusView,
    private readonly workspaceFolder: vscode.WorkspaceFolder,
    private configResource: vscode.Uri,
    ownerKey: string,
    private readonly isActive: () => boolean,
  ) {
    this.clientStatus = new ClientStatus(status, isActive);
    this.owners = new WorkspaceOwners(ownerKey);
  }

  public workspaceKey(): string {
    return this.workspaceFolder.uri.toString();
  }

  public workspaceRoot(): string {
    return this.workspaceFolder.uri.fsPath;
  }

  public workspaceName(): string {
    return this.workspaceFolder.name;
  }

  public isRunning(): boolean {
    return this.client !== undefined;
  }

  public addOwner(ownerKey: string): void {
    this.owners.add(ownerKey);
  }

  public useConfigResource(configResource: vscode.Uri): void {
    this.configResource = configResource;
  }

  public removeOwner(ownerKey: string): void {
    this.owners.delete(ownerKey);
  }

  public hasNoOwners(): boolean {
    return this.owners.isEmpty();
  }

  public ownerKeys(): string[] {
    return this.owners.snapshot();
  }

  public containsDocument(document: vscode.TextDocument): boolean {
    if (!isRustFile(document)) {
      return false;
    }

    const relative = path.relative(this.workspaceFolder.uri.fsPath, document.uri.fsPath);
    return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
  }

  public async start(): Promise<void> {
    if (this.client !== undefined) {
      return;
    }

    const config = ExtensionConfig.read(this.configResource);
    const server = ResolvedServer.discover(config, this.workspaceFolder, this.extensionPath);
    const statusDetails = {
      workspaceRoot: this.workspaceFolder.uri.fsPath,
      serverCommand: ResolvedServer.commandLine(server),
      serverSource: server.source,
    };

    this.output.appendLine(`workspace root: ${this.workspaceFolder.uri.fsPath}`);
    this.output.appendLine(`server command: ${statusDetails.serverCommand}`);
    this.output.appendLine(`server source: ${statusDetails.serverSource}`);
    this.clientStatus.starting(statusDetails);

    const clientOptions: LanguageClientOptions = {
      documentSelector: [
        {
          scheme: "file",
          language: "rust",
          pattern: `${globPath(this.workspaceFolder.uri.fsPath)}/**/*.rs`,
        },
      ],
      diagnosticCollectionName: `rust-glancer-${clientIdSuffix(this.workspaceFolder)}`,
      outputChannel: this.output,
      traceOutputChannel: this.output,
      initializationOptions: {
        check: config.check,
        cargo: config.cargo,
        cache: config.cache,
      },
      middleware: this.middleware(),
      workspaceFolder: this.workspaceFolder,
    };

    const client = new LanguageClient(
      `rust-glancer-${clientIdSuffix(this.workspaceFolder)}`,
      `Rust Glancer (${this.workspaceFolder.name})`,
      ResolvedServer.options(server, this.output),
      clientOptions,
    );

    this.client = client;
    this.clientState = client.onDidChangeState((event) => {
      switch (event.newState) {
        case State.Starting:
          this.clientStatus.starting(statusDetails);
          break;
        case State.Running:
          this.clientStatus.ready(statusDetails);
          this.refreshStatus();
          break;
        case State.Stopped:
          if (this.client === client) {
            this.clientStatus.stopped("language client stopped", statusDetails);
          }
          break;
      }
    });

    try {
      await client.start();
      await client.setTrace(trace(config.traceServer));
      this.clientStatus.ready(statusDetails);
      this.refreshStatus();
      this.output.appendLine("rust-glancer client started");
    } catch (error) {
      this.client = undefined;
      this.clientState?.dispose();
      this.clientState = undefined;
      this.clientStatus.failed(String(error), statusDetails);
      this.output.appendLine(`rust-glancer client failed to start: ${String(error)}`);
      void vscode.window.showErrorMessage(
        "Rust Glancer failed to start. Check the Rust Glancer output for details.",
      );
    }
  }

  public async restart(): Promise<void> {
    this.output.appendLine(`restarting rust-glancer server for ${this.workspaceRoot()}`);
    await this.stop();
    await this.start();
  }

  public async reindexWorkspace(): Promise<void> {
    const client = this.client;
    if (!this.clientStatus.isRunning() || client === undefined) {
      void vscode.window.showWarningMessage("Rust Glancer is not running for this workspace.");
      return;
    }

    this.output.appendLine(`reindexing rust-glancer workspace: ${this.workspaceRoot()}`);
    this.clientStatus.indexing();

    try {
      await client.sendRequest(ExecuteCommandRequest.type, {
        command: SERVER_COMMANDS.reindexWorkspace,
        arguments: [],
      });
      this.output.appendLine(`rust-glancer workspace reindex finished: ${this.workspaceRoot()}`);
      this.refreshStatus();
    } catch (error) {
      this.output.appendLine(`rust-glancer workspace reindex failed: ${String(error)}`);
      this.clientStatus.operationFailed(`reindex failed: ${String(error)}`);
      void vscode.window.showErrorMessage(
        "Rust Glancer failed to reindex the workspace. Check the Rust Glancer output for details.",
      );
    }
  }

  public async stop(): Promise<void> {
    const client = this.client;
    this.client = undefined;
    this.clientState?.dispose();
    this.clientState = undefined;

    if (client !== undefined) {
      await client.stop();
      this.output.appendLine(`rust-glancer client stopped: ${this.workspaceRoot()}`);
    }

    this.clientStatus.stopped("not running");
  }

  public refreshStatus(): void {
    this.clientStatus.refresh(this.isActiveRustDocumentDirty());
  }

  public snapshot(): WorkspaceClientSnapshot {
    const status = this.clientStatus.snapshot();
    return {
      workspaceRoot: this.workspaceRoot(),
      workspaceUri: this.workspaceKey(),
      ownerKeys: this.ownerKeys(),
      hasClient: this.client !== undefined,
      ...status,
    };
  }

  public dispose(): void {
    void this.stop();
  }

  private middleware(): LanguageClientOptions["middleware"] {
    return {
      ...hoverMiddleware(() => this.client, this.output),
      handleWorkDoneProgress: (token, params, next) => {
        this.clientStatus.handleWorkDoneProgress(token, params, this.isActiveRustDocumentDirty());
        next(token, params);
      },
    };
  }

  private isActiveRustDocumentDirty(): boolean {
    const document = vscode.window.activeTextEditor?.document;
    return document !== undefined && this.containsDocument(document) && document.isDirty;
  }
}

function clientIdSuffix(workspaceFolder: vscode.WorkspaceFolder): string {
  return workspaceFolder.uri.toString().replace(/[^A-Za-z0-9_-]+/g, "-");
}

function globPath(path: string): string {
  return path.replace(/\\/g, "/");
}

function trace(setting: TraceSetting): Trace {
  switch (setting) {
    case "off":
      return Trace.Off;
    case "messages":
      return Trace.Messages;
    case "verbose":
      return Trace.Verbose;
  }
}
