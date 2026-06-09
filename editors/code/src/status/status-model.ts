/**
 * Plain status data and rendering helpers shared by the VS Code view and unit-testable state
 * machine. This file intentionally has no `vscode` dependency.
 */
import * as path from "node:path";

export interface StatusDetails {
  readonly workspaceRoot?: string;
  readonly activeWorkspaceRoot?: string;
  readonly serverCommand?: string;
  readonly serverSource?: string;
}

export type StatusState =
  | "created"
  | "starting"
  | "indexing"
  | "ready"
  | "stale"
  | "diagnostics-running"
  | "diagnostics-failed"
  | "stopped"
  | "failed"
  | "disposed";

export interface StatusSnapshot {
  readonly state: StatusState;
  readonly text: string;
  readonly details: StatusDetails;
}

export function statusText(baseText: string, details: StatusDetails | undefined): string {
  const label = workspaceLabel(details);
  return label === undefined ? baseText : `${baseText} [${label}]`;
}

function workspaceLabel(details: StatusDetails | undefined): string | undefined {
  const root = details?.activeWorkspaceRoot;
  if (root === undefined) {
    return undefined;
  }

  const trimmed = root.replace(/[\\/]+$/, "");
  const label = path.basename(trimmed);
  return label.length > 0 ? label : trimmed;
}
