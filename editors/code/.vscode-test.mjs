import { defineConfig } from "@vscode/test-cli";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const extensionRoot = dirname(fileURLToPath(import.meta.url));
const userDataDir = mkdtempSync(resolve(tmpdir(), "rust-glancer-code-user-data-"));
const extensionsDir = mkdtempSync(resolve(tmpdir(), "rust-glancer-code-extensions-"));

export default defineConfig({
  files: "out/test/**/*.test.js",
  version: vscodeTestVersion(),
  extensionDevelopmentPath: extensionRoot,
  workspaceFolder: resolve(extensionRoot, "../../test_targets"),
  env: {
    RUST_GLANCER_VSCODE_USER_DATA_DIR: userDataDir,
  },
  // The rapid-typing test needs renderer key events: programmatic document edits do not exercise
  // VS Code's completion trigger path. Keep the debugger local and let Chromium choose the port.
  launchArgs: [
    "--disable-extensions",
    "--disable-workspace-trust",
    "--enable-smoke-test-driver",
    "--remote-debugging-address=127.0.0.1",
    "--remote-debugging-port=0",
    `--user-data-dir=${userDataDir}`,
    `--extensions-dir=${extensionsDir}`,
  ],
  mocha: {
    timeout: 60_000,
  },
});

function vscodeTestVersion() {
  const packageJsonPath = resolve(extensionRoot, "package.json");
  const packageJson = JSON.parse(readFileSync(packageJsonPath, "utf8"));
  const engine = packageJson.engines?.vscode;
  const version = typeof engine === "string" ? engine.match(/\d+\.\d+\.\d+/)?.[0] : undefined;

  if (version === undefined) {
    throw new Error(`Could not read VS Code engine version from ${packageJsonPath}`);
  }

  // The extension must load on the minimum supported VS Code version. Keeping
  // this derived from package.json makes Renovate engine bumps move the smoke
  // test version along with the manifest.
  return version;
}
