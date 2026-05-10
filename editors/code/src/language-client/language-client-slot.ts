/**
 * Holds the single live language-client session for the VS Code window.
 *
 * VS Code can ask us to start from activation, editor focus, document-open events, or commands at
 * nearly the same time. This slot makes those callers share one initialization attempt without
 * pushing concurrency bookkeeping into `LanguageClientSession` itself.
 */
import * as vscode from "vscode";

import { StatusView } from "../status/status-view";
import { LanguageClientSession } from "./language-client-session";

export class LanguageClientSlot implements vscode.Disposable {
  private session: LanguageClientSession | undefined;
  private initialization: Promise<LanguageClientSession | undefined> | undefined;
  private generation = 0;

  public constructor(
    private readonly extensionPath: string,
    private readonly output: vscode.OutputChannel,
    private readonly status: StatusView,
  ) {}

  public current(): LanguageClientSession | undefined {
    return this.session;
  }

  public async getSession(
    workspaceFolder: vscode.WorkspaceFolder,
  ): Promise<LanguageClientSession | undefined> {
    if (this.session !== undefined) {
      return this.session;
    }

    if (this.initialization !== undefined) {
      return this.initialization;
    }

    const generation = this.generation;
    this.initialization = this.initialize(workspaceFolder, generation);

    try {
      return await this.initialization;
    } finally {
      if (this.generation === generation) {
        this.initialization = undefined;
      }
    }
  }

  public async replace(
    workspaceFolder: vscode.WorkspaceFolder,
  ): Promise<LanguageClientSession | undefined> {
    await this.stop();
    return this.getSession(workspaceFolder);
  }

  public async stop(): Promise<void> {
    this.generation += 1;

    const session = this.session;
    const initialization = this.initialization;
    this.session = undefined;
    this.initialization = undefined;

    await session?.stop();
    await initialization;
  }

  public dispose(): void {
    void this.stop();
  }

  private async initialize(
    workspaceFolder: vscode.WorkspaceFolder,
    generation: number,
  ): Promise<LanguageClientSession | undefined> {
    const session = new LanguageClientSession(
      this.extensionPath,
      this.output,
      this.status,
      workspaceFolder,
    );
    const started = await session.start();

    // Stop/replace can happen while the language client is still starting. If that happens, retire
    // the just-started session instead of publishing stale state back into the slot.
    if (this.generation !== generation) {
      await session.stop();
      return undefined;
    }

    if (!started) {
      return undefined;
    }

    this.session = session;
    return session;
  }
}
