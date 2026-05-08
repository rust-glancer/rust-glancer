/**
 * Reads and normalizes user-facing VS Code settings into runtime configuration.
 *
 * The rest of the extension should consume these typed values instead of repeatedly touching
 * `workspace.getConfiguration`, especially for resource-scoped settings selected by a document or
 * owner folder.
 */
import * as vscode from "vscode";

export type TraceSetting = "off" | "messages" | "verbose";
export type PackageResidencySetting =
  | "all-resident"
  | "workspace"
  | "workspace-and-path-deps"
  | "workspace-path-and-direct-deps"
  | "all-offloadable";

export interface ExtensionConfig {
  readonly serverPath: string | undefined;
  readonly extraEnv: Record<string, string>;
  readonly purgeMemoryAfterBuild: boolean;
  readonly cargo: CargoConfig;
  readonly cache: CacheConfig;
  readonly traceServer: TraceSetting;
  readonly check: CheckConfig;
}

export interface CargoConfig {
  readonly target: string | undefined;
}

export interface CacheConfig {
  readonly packageResidency: PackageResidencySetting;
}

export interface CheckConfig {
  readonly onStartup: boolean;
  readonly onSave: boolean;
  readonly command: string;
  readonly arguments: string[];
}

export namespace ExtensionConfig {
  export function read(resource?: vscode.Uri): ExtensionConfig {
    const config = vscode.workspace.getConfiguration("rust-glancer", resource);
    const serverPath = config.get<string | null>("server.path", null);
    const extraEnv = config.get<Record<string, unknown>>("server.extraEnv", {});
    const purgeMemoryAfterBuild = config.get<boolean>("server.purgeMemoryAfterBuild", true);
    const cargoTarget = config.get<string | null>("cargo.target", null);
    const packageResidency = config.get<PackageResidencySetting>(
      "cache.packageResidency",
      "workspace-and-path-deps",
    );
    const traceServer = config.get<TraceSetting>("trace.server", "off");
    const checkOnStartup = config.get<boolean>("checkOnStartup", false);
    const checkOnSave = config.get<boolean>("checkOnSave", false);
    const checkCommand = config.get<string>("check.command", "check");
    const checkArguments = config.get<unknown[]>("check.arguments", [
      "--workspace",
      "--all-targets",
    ]);

    return {
      serverPath: normalizeOptionalString(serverPath),
      extraEnv: normalizeStringRecord(extraEnv),
      purgeMemoryAfterBuild,
      cargo: {
        target: normalizeOptionalString(cargoTarget),
      },
      cache: {
        packageResidency,
      },
      traceServer,
      check: {
        onStartup: checkOnStartup,
        onSave: checkOnSave,
        command: normalizeCargoSubcommand(checkCommand),
        arguments: normalizeStringArray(checkArguments),
      },
    };
  }
}

function normalizeOptionalString(value: string | null): string | undefined {
  if (typeof value !== "string") {
    return undefined;
  }

  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : undefined;
}

function normalizeStringRecord(value: Record<string, unknown>): Record<string, string> {
  const result: Record<string, string> = {};

  // VS Code settings are user-editable JSON. Keep the runtime boundary strict
  // and ignore malformed entries rather than failing extension activation.
  for (const [key, envValue] of Object.entries(value)) {
    if (typeof envValue === "string") {
      result[key] = envValue;
    }
  }

  return result;
}

function normalizeStringArray(value: unknown[]): string[] {
  return value.filter((item): item is string => typeof item === "string");
}

function normalizeCargoSubcommand(value: string): string {
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : "check";
}
