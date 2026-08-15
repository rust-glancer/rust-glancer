import * as assert from "node:assert/strict";
import * as vscode from "vscode";

import { EXTENSION_COMMANDS } from "../src/commands";
import type { ExtensionControllerSnapshot } from "../src/extension-controller";
import type { LanguageClientSessionSnapshot } from "../src/language-client/language-client-session";
import type { CompletionObservation } from "../src/test-support/completion-observer";

export async function currentCompletionObservations(): Promise<readonly CompletionObservation[]> {
  const state = await vscode.commands.executeCommand<ExtensionControllerSnapshot>(
    EXTENSION_COMMANDS.testGetState,
  );
  return state?.session?.completionObservations ?? [];
}

export async function waitForCompletionObservation(
  after: number,
  isExpected: (observation: CompletionObservation) => boolean,
  description: string,
): Promise<CompletionObservation> {
  const startedAt = Date.now();
  let observations: readonly CompletionObservation[] = [];

  while (Date.now() - startedAt < 10_000) {
    observations = (await currentCompletionObservations()).slice(after);
    const expected = observations.find(isExpected);
    if (expected !== undefined) {
      return expected;
    }

    await delay(5);
  }

  assert.fail(
    `timed out waiting for ${description}; completion observations: ${JSON.stringify(observations)}`,
  );
}

export async function waitForClientState(
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

export async function waitForOutput(pattern: RegExp): Promise<string> {
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

export function readySession(
  state: ExtensionControllerSnapshot,
): LanguageClientSessionSnapshot | undefined {
  const session = state.session;
  if (session?.running === true && session.hasClient && session.status.state === "ready") {
    return session;
  }

  return undefined;
}

export function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
