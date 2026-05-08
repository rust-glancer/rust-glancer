/**
 * Central registry of command identifiers used by the extension and the language server.
 *
 * Keeping command strings here prevents package manifest commands, VS Code registrations, hover
 * links, tests, and LSP execute-command calls from drifting apart.
 */
export const EXTENSION_COMMANDS = {
  restartServer: "rust-glancer.restartServer",
  stopServer: "rust-glancer.stopServer",
  reindexWorkspace: "rust-glancer.reindexWorkspace",
  goToTypeFromHover: "rust-glancer.gotoTypeFromHover",
  testGetState: "rust-glancer.test.getState",
  testGetOutput: "rust-glancer.test.getOutput",
} as const;

export const SERVER_COMMANDS = {
  reindexWorkspace: "rust-glancer.internal.reindexWorkspace",
} as const;
