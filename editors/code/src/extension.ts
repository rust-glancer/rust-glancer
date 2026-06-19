/**
 * VS Code extension entrypoint.
 *
 * This module creates the shared UI objects, registers commands, exposes test-only hooks, and
 * hands runtime coordination to `ExtensionController` during activation/deactivation.
 */
import * as vscode from "vscode";

import { EXTENSION_COMMANDS } from "./commands";
import { ExtensionController } from "./extension-controller";
import { registerHoverActionCommands } from "./features/hover-actions";
import { ServerOutputChannel } from "./logging/server-output-channel";
import { StatusView } from "./status/status-view";
import {
  RecordingLogOutputChannel,
  RecordingOutputChannel,
} from "./test-support/recording-output-channel";

let controller: ExtensionController | undefined;

const EXTENSION_TEST_ENV = "RUST_GLANCER_EXTENSION_TEST";

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const rawExtensionLog = vscode.window.createOutputChannel("Rust Glancer Extension", {
    log: true,
  });
  const rawServerLog = vscode.window.createOutputChannel("Rust Glancer Language Server", "log");
  const recordingExtensionLog =
    process.env[EXTENSION_TEST_ENV] === "1"
      ? new RecordingLogOutputChannel(rawExtensionLog)
      : undefined;
  const recordingServerLog =
    process.env[EXTENSION_TEST_ENV] === "1" ? new RecordingOutputChannel(rawServerLog) : undefined;
  const extensionLog = recordingExtensionLog ?? rawExtensionLog;
  const serverOutput = new ServerOutputChannel(recordingServerLog ?? rawServerLog);
  const status = new StatusView();
  controller = new ExtensionController(extensionLog, serverOutput, status, context.extensionUri);

  // Extension-host tests need a stable synchronization point. Keep this command out of the
  // package manifest so it is available only when the test runner opts in through the environment.
  if (process.env[EXTENSION_TEST_ENV] === "1") {
    context.subscriptions.push(
      vscode.commands.registerCommand(EXTENSION_COMMANDS.testGetState, () =>
        controller?.snapshot(),
      ),
      vscode.commands.registerCommand(EXTENSION_COMMANDS.testGetOutput, () =>
        [recordingExtensionLog?.snapshot(), recordingServerLog?.snapshot()]
          .filter((output): output is string => output !== undefined)
          .join(""),
      ),
    );
  }

  context.subscriptions.push(
    extensionLog,
    serverOutput,
    status,
    controller,
    registerHoverActionCommands(extensionLog),
    vscode.commands.registerCommand(EXTENSION_COMMANDS.restartServer, async () => {
      await controller?.restart();
    }),
    vscode.commands.registerCommand(EXTENSION_COMMANDS.stopServer, async () => {
      await controller?.stopServer();
    }),
    vscode.commands.registerCommand(EXTENSION_COMMANDS.reindexWorkspace, async () => {
      await controller?.reindexWorkspace();
    }),
  );

  await controller.start();
}

export async function deactivate(): Promise<void> {
  await controller?.stop();
  controller = undefined;
}
