// Keep E2E coverage about working user flows. Detailed completion and cancellation contracts
// belong in the engine and transport tests, where their inputs and ordering can be controlled.
import * as assert from "node:assert/strict";
import * as path from "node:path";
import * as vscode from "vscode";

import { EXTENSION_COMMANDS } from "../src/commands";
import { waitFor, withTimeout } from "./async";
import { completeInEditor } from "./completion-scenario";
import {
  assertDefinition,
  clientState,
  serverOutput,
  waitForReadyWorkspace,
} from "./extension-harness";

suite("Rust Glancer extension", () => {
  let projects: vscode.Uri;

  suiteSetup(async () => {
    const extension = vscode.extensions.getExtension("rust-glancer.rust-glancer");
    assert.ok(extension, "VS Code should load Rust Glancer");
    projects = vscode.Uri.file(path.resolve(extension.extensionPath, "../../test_targets"));
    await withTimeout(extension.activate(), "activate Rust Glancer", 30_000);
  });

  teardown(async function () {
    if (this.currentTest?.state === "failed") {
      const evidence = await Promise.allSettled([clientState(), serverOutput()]);
      for (const result of evidence) {
        console.error(result.status === "fulfilled" ? result.value : result.reason);
      }
    }
  });

  suiteTeardown(async () => {
    await withTimeout(
      vscode.commands.executeCommand(EXTENSION_COMMANDS.stopServer),
      "stop test server",
    );
  });

  test("serves both Rust projects through one server and survives reindex and restart", async () => {
    const simple = await vscode.workspace.openTextDocument(
      vscode.Uri.joinPath(projects, "simple_crate", "src", "lib.rs"),
    );
    await vscode.window.showTextDocument(simple);
    await waitForReadyWorkspace("simple_crate");
    await assertDefinition(simple, "|left + right", "|left: i32");

    const commands = await vscode.commands.getCommands(true);
    for (const command of [
      EXTENSION_COMMANDS.showServerActions,
      EXTENSION_COMMANDS.startServer,
      EXTENSION_COMMANDS.restartServer,
      EXTENSION_COMMANDS.stopServer,
      EXTENSION_COMMANDS.reindexWorkspace,
      EXTENSION_COMMANDS.openLogs,
    ]) {
      assert.ok(commands.includes(command), `${command} should be registered`);
    }

    await withTimeout(
      vscode.commands.executeCommand(EXTENSION_COMMANDS.reindexWorkspace),
      "reindex simple_crate",
      30_000,
    );
    await waitForReadyWorkspace("simple_crate");
    await assertDefinition(simple, "|left + right", "|left: i32");

    const moderate = await vscode.workspace.openTextDocument(
      vscode.Uri.joinPath(projects, "moderate_crate", "src", "model.rs"),
    );
    await vscode.window.showTextDocument(moderate);
    await waitForReadyWorkspace("moderate_crate");
    await assertDefinition(moderate, "impl Display for |Note", "pub struct |Note");

    // Switching back must still query the first project without launching another LSP server.
    await vscode.window.showTextDocument(simple);
    await assertDefinition(simple, "|left + right", "|left: i32");
    const output = await serverOutput();
    assert.equal(output.match(/server process started/g)?.length ?? 0, 1, output);

    await withTimeout(
      vscode.commands.executeCommand(EXTENSION_COMMANDS.stopServer),
      "stop Rust Glancer",
    );
    await waitFor(
      "server stopped",
      clientState,
      (state) => state.session === undefined && state.status.state === "stopped",
    );
    await vscode.window.showTextDocument(moderate);
    assert.equal((await clientState()).session, undefined);

    await withTimeout(
      vscode.commands.executeCommand(EXTENSION_COMMANDS.startServer),
      "start Rust Glancer again",
      30_000,
    );
    await waitForReadyWorkspace("moderate_crate");
    await assertDefinition(moderate, "impl Display for |Note", "pub struct |Note");
  });

  test("offers, accepts, and dismisses semantic completions in the editor", async () => {
    const document = await vscode.workspace.openTextDocument(
      vscode.Uri.joinPath(projects, "moderate_crate", "src", "lib.rs"),
    );
    await vscode.window.showTextDocument(document);
    await withTimeout(
      vscode.commands.executeCommand(EXTENSION_COMMANDS.startServer),
      "start completion test server",
      30_000,
    );
    await waitForReadyWorkspace("moderate_crate");
    await completeInEditor(document);
  });
});
