// Note: We don't want to have a ton of tests for the extension.
// These are verbose and heavyweight, so we keep them mostly as a
// smoke test / basic e2e test.

import * as assert from "node:assert/strict";
import * as path from "node:path";
import * as vscode from "vscode";

import { EXTENSION_COMMANDS } from "../src/commands";

const EXTENSION_ID = "rust-glancer.rust-glancer-code";

interface ClientManagerSnapshot {
  readonly activeWorkspaceUri?: string;
  readonly workspaces: WorkspaceClientSnapshot[];
  readonly status: {
    readonly state: string;
    readonly text: string;
  };
}

interface WorkspaceClientSnapshot {
  readonly running: boolean;
  readonly hasClient: boolean;
  readonly workspaceRoot: string;
  readonly workspaceUri: string;
  readonly failureReason?: string;
  readonly status: {
    readonly state: string;
    readonly text: string;
    readonly details: {
      readonly workspaceRoot?: string;
      readonly serverCommand?: string;
      readonly serverSource?: string;
    };
  };
}

suite("Rust Glancer extension", () => {
  test("starts one real server per active Rust workspace", async () => {
    const extension = vscode.extensions.getExtension(EXTENSION_ID);
    assert.ok(extension, `expected VS Code to load extension ${EXTENSION_ID}`);

    await extension.activate();

    const repositoryRoot = path.resolve(extension.extensionPath, "..", "..");
    const testTargetsUri = vscode.Uri.file(path.join(repositoryRoot, "test_targets"));
    const simpleUri = vscode.Uri.joinPath(testTargetsUri, "simple_crate");
    const moderateUri = vscode.Uri.joinPath(testTargetsUri, "moderate_crate");
    ensureWorkspaceFolder(testTargetsUri, "test_targets");

    const simpleDocument = await vscode.workspace.openTextDocument(
      vscode.Uri.joinPath(simpleUri, "src", "lib.rs"),
    );
    await vscode.window.showTextDocument(simpleDocument);

    const simpleReady = await waitForClientState(
      (state) => readyWorkspace(state, /test_targets[/\\]simple_crate$/) !== undefined,
    );
    const simple = readyWorkspace(simpleReady, /test_targets[/\\]simple_crate$/);
    assert.ok(simple);
    assert.equal(simple.hasClient, true);
    assert.equal(simpleReady.status.text, "$(check) Rust Glancer: ready");

    const commands = await vscode.commands.getCommands(true);
    assert.ok(commands.includes(EXTENSION_COMMANDS.restartServer));
    assert.ok(commands.includes(EXTENSION_COMMANDS.stopServer));
    assert.ok(commands.includes(EXTENSION_COMMANDS.reindexWorkspace));

    await vscode.commands.executeCommand(EXTENSION_COMMANDS.reindexWorkspace);

    const reindexed = await waitForClientState(
      (state) => readyWorkspace(state, /test_targets[/\\]simple_crate$/) !== undefined,
    );
    assert.equal(readyWorkspace(reindexed, /test_targets[/\\]simple_crate$/)?.hasClient, true);

    const document = await vscode.workspace.openTextDocument(
      vscode.Uri.joinPath(moderateUri, "src", "lib.rs"),
    );
    await vscode.window.showTextDocument(document);

    const multiRootReady = await waitForClientState(
      (state) => readyWorkspace(state, /test_targets[/\\]moderate_crate$/) !== undefined,
    );
    assert.ok(readyWorkspace(multiRootReady, /test_targets[/\\]simple_crate$/));
    assert.ok(readyWorkspace(multiRootReady, /test_targets[/\\]moderate_crate$/));
    assert.equal(
      multiRootReady.workspaces.length,
      2,
      JSON.stringify(multiRootReady.workspaces, undefined, 2),
    );

    await vscode.commands.executeCommand(EXTENSION_COMMANDS.stopServer);

    const stoppedModerate = await waitForClientState(
      (state) =>
        readyWorkspace(state, /test_targets[/\\]simple_crate$/) !== undefined &&
        readyWorkspace(state, /test_targets[/\\]moderate_crate$/) === undefined &&
        state.workspaces.length === 1,
    );
    assert.equal(stoppedModerate.activeWorkspaceUri, undefined);

    await vscode.window.showTextDocument(simpleDocument);
    await vscode.window.showTextDocument(document);

    const restartedModerate = await waitForClientState(
      (state) => readyWorkspace(state, /test_targets[/\\]moderate_crate$/) !== undefined,
    );
    assert.ok(readyWorkspace(restartedModerate, /test_targets[/\\]simple_crate$/));
    assert.ok(readyWorkspace(restartedModerate, /test_targets[/\\]moderate_crate$/));
  });
});

async function waitForClientState(
  isExpected: (state: ClientManagerSnapshot) => boolean,
): Promise<ClientManagerSnapshot> {
  const startedAt = Date.now();
  let lastState: ClientManagerSnapshot | undefined;

  while (Date.now() - startedAt < 30_000) {
    lastState = await vscode.commands.executeCommand<ClientManagerSnapshot>(
      EXTENSION_COMMANDS.testGetState,
    );
    if (lastState !== undefined && isExpected(lastState)) {
      return lastState;
    }

    await delay(100);
  }

  const output = await vscode.commands.executeCommand<string>(EXTENSION_COMMANDS.testGetOutput);
  assert.fail(
    [
      `timed out waiting for rust-glancer extension state; last state: ${JSON.stringify(lastState)}`,
      "Rust Glancer output:",
      output,
    ].join("\n"),
  );
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function ensureWorkspaceFolder(uri: vscode.Uri, name: string): void {
  const folders = vscode.workspace.workspaceFolders ?? [];
  if (folders.some((folder) => folder.uri.toString() === uri.toString())) {
    return;
  }

  const added = vscode.workspace.updateWorkspaceFolders(folders.length, 0, { uri, name });
  assert.equal(added, true);
}

function readyWorkspace(
  state: ClientManagerSnapshot,
  workspaceRoot: RegExp,
): WorkspaceClientSnapshot | undefined {
  return state.workspaces.find(
    (workspace) =>
      workspace.running &&
      workspace.hasClient &&
      workspace.status.state === "ready" &&
      workspaceRoot.test(workspace.workspaceRoot),
  );
}
