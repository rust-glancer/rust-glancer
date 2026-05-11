// Note: We don't want to have a ton of tests for the extension.
// These are verbose and heavyweight, so we keep them mostly as a
// smoke test / basic e2e test.

import * as assert from "node:assert/strict";
import * as path from "node:path";
import * as vscode from "vscode";

import { EXTENSION_COMMANDS } from "../src/commands";

const EXTENSION_ID = "rust-glancer.rust-glancer-code";

interface ExtensionControllerSnapshot {
  readonly session?: LanguageClientSessionSnapshot;
  readonly status: {
    readonly state: string;
    readonly text: string;
  };
}

interface LanguageClientSessionSnapshot {
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
      readonly activeWorkspaceRoot?: string;
      readonly serverCommand?: string;
      readonly serverSource?: string;
    };
  };
}

suite("Rust Glancer extension", () => {
  test("starts one real server and lets it route multiple Rust workspaces", async () => {
    const extension = vscode.extensions.getExtension(EXTENSION_ID);
    assert.ok(extension, `expected VS Code to load extension ${EXTENSION_ID}`);

    const repositoryRoot = path.resolve(extension.extensionPath, "..", "..");
    const testTargetsUri = vscode.Uri.file(path.join(repositoryRoot, "test_targets"));
    const simpleUri = vscode.Uri.joinPath(testTargetsUri, "simple_crate");
    const moderateUri = vscode.Uri.joinPath(testTargetsUri, "moderate_crate");
    ensureWorkspaceFolder(testTargetsUri, "test_targets");

    await extension.activate();

    const simpleDocument = await vscode.workspace.openTextDocument(
      vscode.Uri.joinPath(simpleUri, "src", "lib.rs"),
    );
    await vscode.window.showTextDocument(simpleDocument);

    const simpleReady = await waitForClientState((state) => readySession(state) !== undefined);
    assert.ok(simpleReady.session);
    await waitForOutput(
      /workspace indexing finished.*simple_crate|simple_crate.*workspace indexing finished/,
    );
    const activeSimple = await waitForClientState(
      (state) =>
        activeWorkspaceName(readySession(state)) === "simple_crate" &&
        state.status.text.includes("[simple_crate]"),
    );
    assert.equal(activeSimple.status.text, "$(check) Rust Glancer: ready [simple_crate]");

    const commands = await vscode.commands.getCommands(true);
    assert.ok(commands.includes(EXTENSION_COMMANDS.restartServer));
    assert.ok(commands.includes(EXTENSION_COMMANDS.stopServer));
    assert.ok(commands.includes(EXTENSION_COMMANDS.reindexWorkspace));

    await vscode.commands.executeCommand(EXTENSION_COMMANDS.reindexWorkspace);

    const reindexed = await waitForClientState((state) => readySession(state) !== undefined);
    assert.equal(readySession(reindexed)?.hasClient, true);

    const document = await vscode.workspace.openTextDocument(
      vscode.Uri.joinPath(moderateUri, "src", "lib.rs"),
    );
    await vscode.window.showTextDocument(document);

    await waitForClientState((state) => readySession(state) !== undefined);
    await waitForOutput(
      /workspace indexing finished.*moderate_crate|moderate_crate.*workspace indexing finished/,
    );
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
    await vscode.window.showTextDocument(document);

    const restartedModerate = await waitForClientState(
      (state) => readySession(state) !== undefined && state.session !== undefined,
    );
    assert.ok(readySession(restartedModerate));
  });
});

async function waitForClientState(
  isExpected: (state: ExtensionControllerSnapshot) => boolean,
): Promise<ExtensionControllerSnapshot> {
  const startedAt = Date.now();
  let lastState: ExtensionControllerSnapshot | undefined;

  while (Date.now() - startedAt < 30_000) {
    lastState = await vscode.commands.executeCommand<ExtensionControllerSnapshot>(
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

async function waitForOutput(pattern: RegExp): Promise<string> {
  const startedAt = Date.now();
  let output = "";

  while (Date.now() - startedAt < 30_000) {
    output = (await vscode.commands.executeCommand<string>(EXTENSION_COMMANDS.testGetOutput)) ?? "";
    if (pattern.test(output)) {
      return output;
    }

    await delay(100);
  }

  assert.fail(`timed out waiting for rust-glancer output ${pattern}; output:\n${output}`);
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

function readySession(
  state: ExtensionControllerSnapshot,
): LanguageClientSessionSnapshot | undefined {
  const session = state.session;
  if (session?.running === true && session.hasClient && session.status.state === "ready") {
    return session;
  }

  return undefined;
}

function activeWorkspaceName(
  session: LanguageClientSessionSnapshot | undefined,
): string | undefined {
  const root = session?.status.details.activeWorkspaceRoot;
  return root === undefined ? undefined : path.basename(root);
}
