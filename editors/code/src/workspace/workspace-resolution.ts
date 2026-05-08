/**
 * Resolves VS Code folders and documents into Cargo workspace identities.
 *
 * The extension needs VS Code's folder/resource information for settings and lifecycle, while the
 * server needs Cargo's workspace root for analysis. This module owns that translation and caches
 * Cargo root discovery.
 */
import * as path from "node:path";
import * as vscode from "vscode";

import { cargoWorkspaceManifest } from "../utils/cargo-utils";
import { nearestCargoManifest } from "../utils/fs-utils";
import { isRustFile } from "../utils/lsp-utils";
import { resolveWorkspaceIdentity } from "./workspace-lifecycle";

export interface ResolvedCargoWorkspace {
  readonly workspaceKey: string;
  readonly cargoFolder: vscode.WorkspaceFolder;
  readonly ownerKey: string;
  readonly configResource: vscode.Uri;
}

interface ResolveDirectoryOptions {
  readonly containingFolder?: vscode.WorkspaceFolder;
  readonly configResource?: vscode.Uri;
}

/**
 * Resolves VS Code folders and documents to the Cargo root used by the language server.
 *
 * VS Code and Cargo answer different questions here: VS Code owns settings and folder lifecycle,
 * while Cargo owns the server root. Keeping that translation in one place prevents the client
 * manager from needing to remember which URI is meant for which purpose.
 */
export class CargoWorkspaceResolver {
  private readonly workspaceManifestByManifest = new Map<string, Promise<string>>();

  public constructor(private readonly output: vscode.OutputChannel) {}

  public clearCache(): void {
    this.workspaceManifestByManifest.clear();
  }

  public async workspaceFolders(): Promise<ResolvedCargoWorkspace[]> {
    const folders = vscode.workspace.workspaceFolders ?? [];
    const cargoFolders: ResolvedCargoWorkspace[] = [];

    for (const folder of folders) {
      const cargoRoot = await this.forDirectory(folder.uri, {
        containingFolder: folder,
        configResource: folder.uri,
      });
      if (cargoRoot !== undefined) {
        cargoFolders.push(cargoRoot);
      }
    }

    return cargoFolders;
  }

  public async forDocument(
    document: vscode.TextDocument,
  ): Promise<ResolvedCargoWorkspace | undefined> {
    if (!isRustFile(document)) {
      return undefined;
    }

    const containingFolder = vscode.workspace.getWorkspaceFolder(document.uri);
    return this.forDirectory(vscode.Uri.file(path.dirname(document.uri.fsPath)), {
      containingFolder,
      configResource: document.uri,
    });
  }

  public async forDirectory(
    directory: vscode.Uri,
    options: ResolveDirectoryOptions = {},
  ): Promise<ResolvedCargoWorkspace | undefined> {
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

    return this.forManifest(manifest, options);
  }

  private async forManifest(
    manifestPath: string,
    options: ResolveDirectoryOptions,
  ): Promise<ResolvedCargoWorkspace> {
    const rootManifest = await this.rootManifestForManifest(manifestPath);
    const root = path.dirname(rootManifest);
    const cargoFolder: vscode.WorkspaceFolder = {
      uri: vscode.Uri.file(root),
      name: path.basename(root),
      index: options.containingFolder?.index ?? 0,
    };
    const configResource =
      options.configResource ?? options.containingFolder?.uri ?? cargoFolder.uri;
    const identity = resolveWorkspaceIdentity({
      cargoRootUri: cargoFolder.uri.toString(),
      containingFolderUri: options.containingFolder?.uri.toString(),
      configResourceUri: configResource.toString(),
    });

    return {
      workspaceKey: identity.workspaceKey,
      cargoFolder,
      ownerKey: identity.ownerKey,
      configResource,
    };
  }

  private async rootManifestForManifest(manifestPath: string): Promise<string> {
    const existing = this.workspaceManifestByManifest.get(manifestPath);
    if (existing !== undefined) {
      return existing;
    }

    const resolved = cargoWorkspaceManifest(manifestPath).catch((error) => {
      this.workspaceManifestByManifest.delete(manifestPath);
      // Root discovery is an extension convenience, not the source of truth. If Cargo is missing or
      // `locate-project` fails for a partially edited manifest, the server can still try the nearest
      // manifest and report the real initialization error if that is not enough.
      this.output.appendLine(
        `cargo workspace root discovery failed for ${manifestPath}: ${String(error)}`,
      );
      return manifestPath;
    });
    this.workspaceManifestByManifest.set(manifestPath, resolved);
    return resolved;
  }
}
