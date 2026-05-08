/**
 * Filesystem helpers that need VS Code's workspace filesystem API.
 *
 * The extension uses these helpers when it must respect VS Code URI handling while still doing
 * path-based Cargo discovery and containment checks.
 */
import * as path from "node:path";
import * as vscode from "vscode";

/**
 * Finds the closest `Cargo.toml` by walking from a directory toward the filesystem root.
 *
 * The optional boundary is inclusive: a manifest at the boundary can still be returned, but the
 * search will not climb above that path. This keeps project-catalog windows from escaping into a
 * parent repository that merely happens to contain another `Cargo.toml`.
 */
export async function nearestCargoManifest(
  directory: string,
  boundary: string | undefined,
): Promise<string | undefined> {
  let current = path.resolve(directory);
  const resolvedBoundary = boundary === undefined ? undefined : path.resolve(boundary);

  while (true) {
    const manifest = path.join(current, "Cargo.toml");
    if (await isFile(manifest)) {
      return manifest;
    }

    if (resolvedBoundary !== undefined && samePath(current, resolvedBoundary)) {
      return undefined;
    }

    const parent = path.dirname(current);
    if (samePath(parent, current)) {
      return undefined;
    }
    current = parent;
  }
}

export async function isFile(filePath: string): Promise<boolean> {
  try {
    const stat = await vscode.workspace.fs.stat(vscode.Uri.file(filePath));
    return stat.type === vscode.FileType.File;
  } catch {
    return false;
  }
}

export function isPathInside(child: string, parent: string): boolean {
  const relative = path.relative(path.resolve(parent), path.resolve(child));
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

function samePath(left: string, right: string): boolean {
  return path.resolve(left) === path.resolve(right);
}
