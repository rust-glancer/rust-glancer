/**
 * Owns one started VS Code language-client session.
 *
 * A session has immutable startup details: server command, cwd, initialization options, and
 * workspace folder list. Restarting creates a new session rather than mutating this one.
 */
import * as vscode from "vscode";
import {
  ExecuteCommandRequest,
  LanguageClient,
  State,
  type LanguageClientOptions,
  Trace,
} from "vscode-languageclient/node";

import { SERVER_COMMANDS, SERVER_NOTIFICATIONS } from "../commands";
import { ExtensionConfig, type TraceSetting } from "../config";
import { hoverMiddleware } from "../features/hover-actions";
import {
  ClientStatus,
  type ActiveWorkspaceState,
  type ClientStatusSnapshot,
} from "../status/client-status";
import { StatusView } from "../status/status-view";
import { isRustFile } from "../utils/lsp-utils";
import { ResolvedServer } from "./server";

export interface LanguageClientSessionSnapshot extends ClientStatusSnapshot {
  readonly workspaceRoot: string;
  readonly workspaceUri: string;
  readonly hasClient: boolean;
}

export class LanguageClientSession implements vscode.Disposable {
  private client: LanguageClient | undefined;
  private clientState: vscode.Disposable | undefined;
  private readonly clientStatus: ClientStatus;

  public constructor(
    private readonly extensionPath: string,
    private readonly output: vscode.OutputChannel,
    status: StatusView,
    private readonly workspaceFolder: vscode.WorkspaceFolder,
  ) {
    this.clientStatus = new ClientStatus(status);
  }

  public workspaceKey(): string {
    return this.workspaceFolder.uri.toString();
  }

  public workspaceRoot(): string {
    return this.workspaceFolder.uri.fsPath;
  }

  public isRunning(): boolean {
    return this.client !== undefined;
  }

  public async start(): Promise<boolean> {
    if (this.client !== undefined) {
      return this.clientStatus.isRunning();
    }

    const config = ExtensionConfig.read();
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
      documentSelector: [{ scheme: "file", language: "rust" }],
      diagnosticCollectionName: "rust-glancer",
      outputChannel: this.output,
      traceOutputChannel: this.output,
      initializationOptions: {
        diagnostics: config.diagnostics,
        cargo: config.cargo,
        cache: config.cache,
      },
      middleware: this.middleware(),
    };

    const client = new LanguageClient(
      "rust-glancer",
      "Rust Glancer",
      ResolvedServer.options(server, this.output),
      clientOptions,
    );

    this.client = client;
    this.clientState = vscode.Disposable.from(
      client.onDidChangeState((event) => {
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
      }),
      client.onNotification(SERVER_NOTIFICATIONS.activeWorkspaceChanged, (params) => {
        const status = params as ActiveWorkspaceChangedParams;
        this.clientStatus.activeWorkspace(
          status.root,
          status.state,
          status.message,
          this.isActiveRustDocumentDirty(),
        );
      }),
    );

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
      return false;
    }

    return true;
  }

  public async reindexWorkspace(): Promise<void> {
    const client = this.client;
    if (!this.clientStatus.isRunning() || client === undefined) {
      void vscode.window.showWarningMessage("Rust Glancer is not running.");
      return;
    }

    this.output.appendLine("reindexing rust-glancer active workspace");
    this.clientStatus.indexing();

    try {
      await client.sendRequest(ExecuteCommandRequest.type, {
        command: SERVER_COMMANDS.reindexWorkspace,
        arguments: [],
      });
      this.output.appendLine("rust-glancer active workspace reindex finished");
      this.refreshStatus();
    } catch (error) {
      this.output.appendLine(`rust-glancer active workspace reindex failed: ${String(error)}`);
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
      this.output.appendLine("rust-glancer client stopped");
    }

    this.clientStatus.stopped("not running");
  }

  public refreshStatus(): void {
    this.clientStatus.refresh(this.isActiveRustDocumentDirty());
  }

  public snapshot(): LanguageClientSessionSnapshot {
    const status = this.clientStatus.snapshot();
    return {
      workspaceRoot: this.workspaceRoot(),
      workspaceUri: this.workspaceKey(),
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
    return document !== undefined && isRustFile(document) && document.isDirty;
  }
}

interface ActiveWorkspaceChangedParams {
  readonly root: string;
  readonly state: ActiveWorkspaceState;
  readonly message?: string;
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
