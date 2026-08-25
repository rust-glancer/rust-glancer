#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const extensionRoot = resolve(scriptDir, "..");
const workspaceRoot = resolve(extensionRoot, "../..");
const executableName = process.platform === "win32" ? "rust-glancer.exe" : "rust-glancer";
const server = join(workspaceRoot, "target", "release", executableName);
const testCli = join(extensionRoot, "node_modules", "@vscode", "test-cli", "out", "bin.mjs");

if (!existsSync(server)) {
  fail(`Expected server binary does not exist: ${server}`);
}
if (!existsSync(testCli)) {
  fail(`Expected local VS Code test CLI does not exist: ${testCli}. Run npm install.`);
}

const result = spawnSync(process.execPath, [testCli, ...process.argv.slice(2)], {
  cwd: extensionRoot,
  env: {
    ...process.env,
    RUST_GLANCER_EXTENSION_TEST: "1",
    __RUST_GLANCER_SERVER: server,
  },
  stdio: "inherit",
});

if (result.error !== undefined) {
  fail(result.error.message);
}
if (result.signal !== null) {
  fail(`VS Code test CLI terminated by signal ${result.signal}.`);
}
if (result.status !== 0) {
  fail(`VS Code test CLI exited with status ${result.status}.`);
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
