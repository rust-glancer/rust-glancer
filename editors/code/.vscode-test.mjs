import { defineConfig } from "@vscode/test-cli";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const extensionRoot = dirname(fileURLToPath(import.meta.url));
const userDataDir = mkdtempSync(resolve(tmpdir(), "rust-glancer-code-user-data-"));
const extensionsDir = mkdtempSync(resolve(tmpdir(), "rust-glancer-code-extensions-"));

export default defineConfig({
  files: "out/test/**/*.test.js",
  version: "1.119.0",
  extensionDevelopmentPath: extensionRoot,
  workspaceFolder: resolve(extensionRoot, "../../test_targets"),
  launchArgs: [
    "--disable-extensions",
    "--disable-workspace-trust",
    `--user-data-dir=${userDataDir}`,
    `--extensions-dir=${extensionsDir}`,
  ],
  mocha: {
    timeout: 60_000,
  },
});
