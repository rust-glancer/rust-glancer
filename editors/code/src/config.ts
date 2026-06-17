/**
 * Reads and normalizes user-facing VS Code settings into runtime configuration.
 *
 * The rest of the extension should consume these typed values instead of repeatedly touching
 * `workspace.getConfiguration`. The values are window-level because one LSP server initializes all
 * project engines in the current VS Code window.
 */
import * as vscode from "vscode";

export type PackageResidencySetting =
  | "all-resident"
  | "workspace"
  | "workspace-and-path-deps"
  | "workspace-path-and-direct-deps"
  | "all-offloadable";

export type IndexingPerformancePreferenceSetting = "lower-peak-memory" | "faster-builds";

export interface ExtensionConfig {
  readonly serverPath: string | undefined;
  readonly extraEnv: Record<string, string>;
  readonly purgeMemoryAfterBuild: boolean;
  readonly cfg: CfgConfig;
  readonly indexing: IndexingConfig;
  readonly cargo: CargoConfig;
  readonly cache: CacheConfig;
  readonly diagnostics: DiagnosticsConfig;
}

export interface CfgConfig {
  readonly test: boolean;
}

export interface IndexingConfig {
  readonly performancePreference: IndexingPerformancePreferenceSetting;
}

export interface CargoConfig {
  readonly target: string | undefined;
  readonly allFeatures: boolean;
  readonly noDefaultFeatures: boolean;
  readonly features: string[];
}

export interface CacheConfig {
  readonly packageResidency: PackageResidencySetting;
}

export interface DiagnosticsConfig {
  readonly onStartup: boolean;
  readonly onSave: boolean;
  readonly command: string;
  readonly arguments: string[];
}

export namespace ExtensionConfig {
  export function read(): ExtensionConfig {
    const config = vscode.workspace.getConfiguration("rust-glancer");
    const serverPath = config.get<string | null>("server.path", null);
    const extraEnv = config.get<Record<string, unknown>>("server.extraEnv", {});
    const purgeMemoryAfterBuild = config.get<boolean>("server.purgeMemoryAfterBuild", true);
    const cfgTest = config.get<boolean>("cfg.test", false);
    const indexingPerformancePreference = config.get<IndexingPerformancePreferenceSetting>(
      "indexing.performancePreference",
      "faster-builds",
    );
    const cargoTarget = config.get<string | null>("cargo.target", null);
    const cargoAllFeatures = config.get<boolean>("cargo.allFeatures", false);
    const cargoNoDefaultFeatures = config.get<boolean>("cargo.noDefaultFeatures", false);
    const cargoFeatures = config.get<unknown[]>("cargo.features", []);
    const packageResidency = config.get<PackageResidencySetting>(
      "cache.packageResidency",
      "workspace-and-path-deps",
    );
    const diagnosticsOnStartup = config.get<boolean>("diagnosticsOnStartup", false);
    const diagnosticsOnSave = config.get<boolean>("diagnosticsOnSave", false);
    const diagnosticsCommand = config.get<string>("diagnostics.command", "check");
    const diagnosticsArguments = config.get<unknown[]>("diagnostics.arguments", [
      "--workspace",
      "--all-targets",
    ]);

    return {
      serverPath: normalizeOptionalString(serverPath),
      extraEnv: normalizeStringRecord(extraEnv),
      purgeMemoryAfterBuild,
      cfg: {
        test: cfgTest,
      },
      indexing: {
        performancePreference: indexingPerformancePreference,
      },
      cargo: {
        target: normalizeOptionalString(cargoTarget),
        allFeatures: cargoAllFeatures,
        noDefaultFeatures: cargoNoDefaultFeatures,
        features: normalizeCargoFeatures(cargoFeatures),
      },
      cache: {
        packageResidency,
      },
      diagnostics: {
        onStartup: diagnosticsOnStartup,
        onSave: diagnosticsOnSave,
        command: normalizeCargoSubcommand(diagnosticsCommand),
        arguments: normalizeStringArray(diagnosticsArguments),
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

function normalizeCargoFeatures(value: unknown[]): string[] {
  const features: string[] = [];
  const seen = new Set<string>();

  for (const item of normalizeStringArray(value)) {
    const feature = item.trim();
    if (feature.length === 0 || seen.has(feature)) {
      continue;
    }

    features.push(feature);
    seen.add(feature);
  }

  return features;
}

function normalizeCargoSubcommand(value: string): string {
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : "check";
}
