/**
 * Small Cargo command adapters used by the VS Code client.
 *
 * These utilities keep subprocess details and Cargo-specific error handling away from workspace
 * resolution, which only needs the resulting manifest path.
 */
import { execFile } from "node:child_process";
import * as path from "node:path";

/**
 * Resolves the root workspace manifest for any Cargo manifest path.
 *
 * Member crates can sit below a larger workspace, so the nearest `Cargo.toml` is only a candidate.
 * `cargo locate-project --workspace` asks Cargo for the final root that the server should index.
 */
export async function cargoWorkspaceManifest(manifestPath: string): Promise<string> {
  const output = await execFileText(
    "cargo",
    ["locate-project", "--workspace", "--message-format", "plain", "--manifest-path", manifestPath],
    path.dirname(manifestPath),
  );
  const resolvedManifest = output.trim();
  if (resolvedManifest.length === 0) {
    throw new Error("cargo locate-project returned an empty manifest path");
  }
  return resolvedManifest;
}

/**
 * Runs a subprocess and returns stdout as text.
 *
 * Rejections prefer stderr when available because command-line tools usually put their actionable
 * explanation there, while `error.message` often only says that the process exited with a code.
 */
function execFileText(command: string, args: string[], cwd: string): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(command, args, { cwd }, (error, stdout, stderr) => {
      if (error !== null) {
        reject(stderr.trim().length > 0 ? stderr.trim() : error.message);
        return;
      }
      resolve(stdout);
    });
  });
}
