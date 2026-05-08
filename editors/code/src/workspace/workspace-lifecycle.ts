/**
 * Pure workspace lifecycle rules that do not depend on VS Code runtime objects.
 *
 * These helpers define identity, ownership, and restart-target policy for Cargo workspaces.
 */
export interface WorkspaceIdentityInput {
  readonly cargoRootUri: string;
  readonly containingFolderUri?: string;
  readonly configResourceUri?: string;
}

export interface WorkspaceIdentity {
  /** Key for the running language-client map. This is always the resolved Cargo root. */
  readonly workspaceKey: string;

  /** VS Code folder that currently justifies keeping the Cargo-root client alive. */
  readonly ownerKey: string;

  /** Resource used for VS Code's resource-scoped extension settings. */
  readonly configResourceUri: string;
}

/**
 * Names the three different URIs involved in one workspace client's lifecycle.
 *
 * They usually point at the same directory, but they diverge when the user opens a Cargo member
 * crate and Cargo resolves the language-server root to a parent workspace. The server root must
 * remain the parent, while settings and folder-removal behavior must still follow the opened
 * folder or active document.
 */
export function resolveWorkspaceIdentity(input: WorkspaceIdentityInput): WorkspaceIdentity {
  return {
    workspaceKey: input.cargoRootUri,
    ownerKey: input.containingFolderUri ?? input.cargoRootUri,
    configResourceUri: input.configResourceUri ?? input.containingFolderUri ?? input.cargoRootUri,
  };
}

/**
 * Tracks which VS Code folders keep a Cargo-root client alive.
 *
 * A single Cargo workspace client may be shared by several opened member folders that all resolve
 * to the same parent root. Removing one member folder should only release that owner; the client
 * stops when the last owner disappears.
 */
export class WorkspaceOwners {
  private readonly owners = new Set<string>();

  public constructor(initialOwner: string) {
    this.add(initialOwner);
  }

  public add(ownerKey: string): void {
    this.owners.add(ownerKey);
  }

  public delete(ownerKey: string): void {
    this.owners.delete(ownerKey);
  }

  public isEmpty(): boolean {
    return this.owners.size === 0;
  }

  public snapshot(): string[] {
    return [...this.owners].sort();
  }
}

export type RestartTargetPlan =
  | { readonly kind: "restart-existing"; readonly workspaceKey: string }
  | { readonly kind: "start-active"; readonly workspaceKey: string }
  | { readonly kind: "start-discovered" }
  | { readonly kind: "restart-single"; readonly workspaceKey: string }
  | { readonly kind: "prompt" };

export interface RestartTargetPlanInput {
  readonly activeWorkspaceKey?: string;
  readonly existingWorkspaceKeys: readonly string[];
}

/**
 * Chooses what "Restart Server" should do without causing an accidental double startup.
 *
 * If the active Rust file belongs to a workspace that is not running yet, the command should start
 * it once. Existing clients are the only targets that should go through stop-then-start restart.
 */
export function planRestartTarget(input: RestartTargetPlanInput): RestartTargetPlan {
  if (input.activeWorkspaceKey !== undefined) {
    return input.existingWorkspaceKeys.includes(input.activeWorkspaceKey)
      ? { kind: "restart-existing", workspaceKey: input.activeWorkspaceKey }
      : { kind: "start-active", workspaceKey: input.activeWorkspaceKey };
  }

  if (input.existingWorkspaceKeys.length === 0) {
    return { kind: "start-discovered" };
  }

  if (input.existingWorkspaceKeys.length === 1) {
    return {
      kind: "restart-single",
      workspaceKey: input.existingWorkspaceKeys[0],
    };
  }

  return { kind: "prompt" };
}
