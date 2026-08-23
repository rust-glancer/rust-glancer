#!/usr/bin/env node

// Launches the VS Code integration test suite with the server binary override prepared in a
// cross-platform way. The npm script cannot do this inline because environment-prefix syntax
// (`VAR=x cmd`) and `$(pwd)` are shell-specific and fail on Windows.
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const extensionRoot = resolve(scriptDir, "..");
const workspaceRoot = resolve(extensionRoot, "../..");

const executableName = process.platform === "win32" ? "rust-glancer.exe" : "rust-glancer";
const serverBinary = join(workspaceRoot, "target", "release", executableName);

if (!existsSync(serverBinary)) {
  console.error(`Expected server binary does not exist: ${serverBinary}`);
  console.error("Build it first with: cargo build --release -p rust-glancer");
  process.exit(1);
}

// The test runner is invoked through its JS entry instead of the `.bin` shim: Node refuses to
// spawn `.cmd` shims without a shell on Windows (CVE-2024-27980 mitigation).
const vscodeTestEntry = join(
  extensionRoot,
  "node_modules",
  "@vscode",
  "test-cli",
  "out",
  "bin.mjs",
);

if (!existsSync(vscodeTestEntry)) {
  console.error(`Expected local vscode-test entry does not exist: ${vscodeTestEntry}`);
  console.error("Run npm install in editors/code.");
  process.exit(1);
}

const result = spawnSync(process.execPath, [vscodeTestEntry], {
  stdio: "inherit",
  cwd: extensionRoot,
  env: {
    ...process.env,
    RUST_GLANCER_EXTENSION_TEST: "1",
    __RUST_GLANCER_SERVER: serverBinary,
  },
});

if (result.error !== undefined) {
  console.error(result.error.message);
  process.exit(1);
}
process.exit(result.status ?? 1);
