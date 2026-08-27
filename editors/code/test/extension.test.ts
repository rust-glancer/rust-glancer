// These two acceptance tests start a real extension host and language server. Keep the suite
// small: one test covers extension/server lifecycle, while one covers native completion behavior.

import * as assert from "node:assert/strict";
import * as path from "node:path";
import * as vscode from "vscode";

import { EXTENSION_COMMANDS } from "../src/commands";
import { CompletionScenario } from "./completion-scenario";
import { readySession, waitForClientState, waitForOutput } from "./extension-harness";

const EXTENSION_ID = "rust-glancer.rust-glancer";

suite("Rust Glancer extension", () => {
  test("starts one real server and routes multiple Rust workspaces", async () => {
    const extension = rustGlancerExtension();
    const projects = projectUris(extension);
    ensureWorkspaceFolder(projects.testTargets, "test_targets");

    await extension.activate();

    const simpleDocument = await vscode.workspace.openTextDocument(
      vscode.Uri.joinPath(projects.simple, "src", "lib.rs"),
    );
    await vscode.window.showTextDocument(simpleDocument);

    const simpleReady = await waitForClientState((state) => readySession(state) !== undefined);
    assert.ok(simpleReady.session);
    await waitForOutput(workspaceIndexingFinishedPattern("simple_crate"));
    const activeSimple = await waitForClientState(
      (state) =>
        activeWorkspaceName(readySession(state)) === "simple_crate" &&
        state.status.text.includes("[simple_crate]"),
    );
    assert.match(
      activeSimple.status.text,
      /^(?:~|\$\(check\)) Rust Glancer: ready \[simple_crate\]$/,
    );

    const commands = await vscode.commands.getCommands(true);
    assert.ok(commands.includes(EXTENSION_COMMANDS.showServerActions));
    assert.ok(commands.includes(EXTENSION_COMMANDS.startServer));
    assert.ok(commands.includes(EXTENSION_COMMANDS.restartServer));
    assert.ok(commands.includes(EXTENSION_COMMANDS.stopServer));
    assert.ok(commands.includes(EXTENSION_COMMANDS.reindexWorkspace));
    assert.ok(commands.includes(EXTENSION_COMMANDS.openLogs));

    await vscode.commands.executeCommand(EXTENSION_COMMANDS.reindexWorkspace);
    const reindexed = await waitForClientState((state) => readySession(state) !== undefined);
    assert.equal(readySession(reindexed)?.hasClient, true);

    const moderateDocument = await vscode.workspace.openTextDocument(
      vscode.Uri.joinPath(projects.moderate, "src", "lib.rs"),
    );
    await vscode.window.showTextDocument(moderateDocument);

    await waitForClientState((state) => readySession(state) !== undefined);
    await waitForOutput(workspaceIndexingFinishedPattern("moderate_crate"));
    const multiRootReady = await waitForClientState(
      (state) =>
        activeWorkspaceName(readySession(state)) === "moderate_crate" &&
        state.status.text.includes("[moderate_crate]"),
    );
    const multiRootSession = readySession(multiRootReady);
    assert.ok(multiRootSession);
    assert.equal(multiRootSession.status.details.workspaceRoot, multiRootSession.workspaceRoot);

    const outputAfterProjectSwitch =
      (await vscode.commands.executeCommand<string>(EXTENSION_COMMANDS.testGetOutput)) ?? "";
    assert.equal(
      outputAfterProjectSwitch.match(/server process started/g)?.length ?? 0,
      1,
      outputAfterProjectSwitch,
    );

    await vscode.commands.executeCommand(EXTENSION_COMMANDS.stopServer);
    await waitForClientState(
      (state) => state.session === undefined && state.status.state === "stopped",
    );

    await vscode.window.showTextDocument(simpleDocument);
    await vscode.window.showTextDocument(moderateDocument);
    const stillStopped = await vscode.commands.executeCommand<ExtensionControllerState>(
      EXTENSION_COMMANDS.testGetState,
    );
    assert.equal(stillStopped?.session, undefined);
    assert.equal(stillStopped?.status.state, "stopped");

    await vscode.commands.executeCommand(EXTENSION_COMMANDS.startServer);
    const restartedModerate = await waitForClientState(
      (state) => readySession(state) !== undefined && state.session !== undefined,
    );
    assert.ok(readySession(restartedModerate));
  });

  test("keeps rapid completions semantic and respects explicit dismissal", async () => {
    const extension = rustGlancerExtension();
    const projects = projectUris(extension);
    ensureWorkspaceFolder(projects.testTargets, "test_targets");
    await extension.activate();

    const document = await vscode.workspace.openTextDocument(
      vscode.Uri.joinPath(projects.moderate, "src", "lib.rs"),
    );
    await vscode.window.showTextDocument(document);
    await waitForOutput(workspaceIndexingFinishedPattern("moderate_crate"));
    await waitForClientState(
      (state) =>
        readySession(state) !== undefined &&
        activeWorkspaceName(readySession(state)) === "moderate_crate",
    );

    await CompletionScenario.run(document);
  });
});

interface ProjectUris {
  readonly testTargets: vscode.Uri;
  readonly simple: vscode.Uri;
  readonly moderate: vscode.Uri;
}

interface ExtensionControllerState {
  readonly status: {
    readonly state: string;
  };
  readonly session: unknown;
}

function rustGlancerExtension(): vscode.Extension<unknown> {
  const extension = vscode.extensions.getExtension(EXTENSION_ID);
  assert.ok(extension, `expected VS Code to load extension ${EXTENSION_ID}`);
  return extension;
}

function projectUris(extension: vscode.Extension<unknown>): ProjectUris {
  const repositoryRoot = path.resolve(extension.extensionPath, "..", "..");
  const testTargets = vscode.Uri.file(path.join(repositoryRoot, "test_targets"));
  return {
    testTargets,
    simple: vscode.Uri.joinPath(testTargets, "simple_crate"),
    moderate: vscode.Uri.joinPath(testTargets, "moderate_crate"),
  };
}

function workspaceIndexingFinishedPattern(workspaceName: string): RegExp {
  return new RegExp(
    `workspace (?:early-start )?indexing finished.*${workspaceName}|${workspaceName}.*workspace (?:early-start )?indexing finished`,
  );
}

function ensureWorkspaceFolder(uri: vscode.Uri, name: string): void {
  const folders = vscode.workspace.workspaceFolders ?? [];
  if (folders.some((folder) => folder.uri.toString() === uri.toString())) {
    return;
  }

  const added = vscode.workspace.updateWorkspaceFolders(folders.length, 0, { uri, name });
  assert.equal(added, true);
}

function activeWorkspaceName(session: ReturnType<typeof readySession>): string | undefined {
  const root = session?.status.details.activeWorkspaceRoot;
  return root === undefined ? undefined : path.basename(root);
}
