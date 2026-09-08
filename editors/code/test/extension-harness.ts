import * as assert from "node:assert/strict";
import * as path from "node:path";
import * as vscode from "vscode";

import { EXTENSION_COMMANDS } from "../src/commands";
import type { ExtensionControllerSnapshot } from "../src/extension-controller";
import { waitFor, withTimeout } from "./async";

export async function clientState(): Promise<ExtensionControllerSnapshot> {
  const state = await withTimeout(
    vscode.commands.executeCommand<ExtensionControllerSnapshot>(EXTENSION_COMMANDS.testGetState),
    "read extension state",
  );
  assert.ok(state, "extension test commands should be registered");
  return state;
}

export async function serverOutput(): Promise<string> {
  const output = await withTimeout(
    vscode.commands.executeCommand<string>(EXTENSION_COMMANDS.testGetOutput),
    "read server output",
  );
  assert.equal(typeof output, "string", "extension test output should be available");
  return output;
}

export async function waitForReadyWorkspace(name: string): Promise<void> {
  // Readiness comes from the active engine's status, so a log line from an earlier server
  // session cannot make a restarted workspace appear ready.
  await waitFor(
    `${name} ready`,
    clientState,
    ({ session }) =>
      session?.running === true &&
      session.hasClient &&
      session.status.state === "ready" &&
      path.basename(session.status.details.activeWorkspaceRoot ?? "") === name,
    30_000,
  );
}

/** A real query makes the routing check independent of status messages and indexing logs. */
export async function assertDefinition(
  document: vscode.TextDocument,
  reference: string,
  declaration: string,
): Promise<void> {
  // `|` marks the symbol inside a short source fragment, without depending on line numbers or
  // the checkout's line endings. Both fragments are read from the same fixture document.
  const position = (marked: string): vscode.Position => {
    const marker = marked.indexOf("|");
    assert.ok(marker >= 0, "query fragment should mark its position with |");
    const offset = document.getText().indexOf(marked.replace("|", ""));
    assert.ok(offset >= 0, `fixture should contain ${marked}`);
    return document.positionAt(offset + marker);
  };
  const definitions = await withTimeout(
    vscode.commands.executeCommand<(vscode.Location | vscode.LocationLink)[]>(
      "vscode.executeDefinitionProvider",
      document.uri,
      position(reference),
    ),
    `definition of ${reference}`,
  );
  const expectedPosition = position(declaration);
  assert.ok(
    definitions?.some((definition) => {
      const uri = "targetUri" in definition ? definition.targetUri : definition.uri;
      const range =
        "targetUri" in definition
          ? (definition.targetSelectionRange ?? definition.targetRange)
          : definition.range;
      return uri.toString() === document.uri.toString() && range.contains(expectedPosition);
    }),
    `${reference} should resolve to its declaration in ${document.uri.fsPath}: ${JSON.stringify(definitions)}`,
  );
}
