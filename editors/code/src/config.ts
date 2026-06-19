/**
 * Reads and normalizes user-facing VS Code settings into runtime configuration.
 *
 * The rest of the extension should consume these typed values instead of repeatedly touching
 * `workspace.getConfiguration`. The values are window-level because one LSP server initializes all
 * project engines in the current VS Code window.
 */
import * as vscode from "vscode";

const PACKAGE_RESIDENCY_VALUES = [
  "all-resident",
  "workspace",
  "workspace-and-path-deps",
  "workspace-path-and-direct-deps",
  "all-offloadable",
] as const;

export type PackageResidencySetting = (typeof PACKAGE_RESIDENCY_VALUES)[number];

const INDEXING_PERFORMANCE_PREFERENCE_VALUES = ["lower-peak-memory", "faster-builds"] as const;

export type IndexingPerformancePreferenceSetting =
  (typeof INDEXING_PERFORMANCE_PREFERENCE_VALUES)[number];

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
  readonly atoms: string[];
}

export interface IndexingConfig {
  readonly performancePreference: IndexingPerformancePreferenceSetting;
}

export interface CargoConfig {
  readonly target: string | undefined;
  readonly allFeatures: boolean;
  readonly noDefaultFeatures: boolean;
  readonly features: string[];
  readonly overrides: CargoOverrideConfig[];
}

export interface CargoOverrideConfig {
  readonly path: string;
  readonly target?: string | null;
  readonly allFeatures?: boolean;
  readonly noDefaultFeatures?: boolean;
  readonly features?: string[];
}

type MutableCargoOverrideConfig = {
  path: string;
  target?: string | null;
  allFeatures?: boolean;
  noDefaultFeatures?: boolean;
  features?: string[];
};

export interface CacheConfig {
  readonly packageResidency: PackageResidencySetting;
}

export interface DiagnosticsConfig {
  readonly onStartup: boolean;
  readonly onSave: boolean;
  readonly command: string;
  readonly cargoArguments: string[];
  readonly extraEnv: Record<string, string>;
}

export namespace ExtensionConfig {
  export function read(): ExtensionConfig {
    const config = vscode.workspace.getConfiguration("rust-glancer");

    return {
      serverPath: normalizeOptionalString(readStringOrNull(config, "server.path", null)),
      extraEnv: normalizeStringRecord(readUnknownRecord(config, "server.extraEnv")),
      purgeMemoryAfterBuild: readBoolean(config, "server.purgeMemoryAfterBuild", true),
      cfg: {
        test: readBoolean(config, "cfg.test", true),
        atoms: normalizeCfgAtoms(readUnknownArray(config, "cfg.atoms")),
      },
      indexing: {
        performancePreference: readStringEnum(
          config,
          "indexing.performancePreference",
          INDEXING_PERFORMANCE_PREFERENCE_VALUES,
          "faster-builds",
        ),
      },
      cargo: {
        target: normalizeOptionalString(readStringOrNull(config, "cargo.target", null)),
        allFeatures: readBoolean(config, "cargo.allFeatures", false),
        noDefaultFeatures: readBoolean(config, "cargo.noDefaultFeatures", false),
        features: normalizeCargoFeatures(readUnknownArray(config, "cargo.features")),
        overrides: normalizeCargoOverrides(readUnknownArray(config, "cargo.overrides")),
      },
      cache: {
        packageResidency: readStringEnum(
          config,
          "cache.packageResidency",
          PACKAGE_RESIDENCY_VALUES,
          "workspace-and-path-deps",
        ),
      },
      diagnostics: {
        onStartup: readBoolean(config, "diagnostics.onStartup", false),
        onSave: readBoolean(config, "diagnostics.onSave", false),
        command: normalizeCargoSubcommand(readString(config, "diagnostics.command", "check")),
        cargoArguments: normalizeStringArray(
          readUnknownArray(config, "diagnostics.cargoArguments", ["--workspace"]),
        ),
        extraEnv: normalizeStringRecord(readUnknownRecord(config, "diagnostics.extraEnv")),
      },
    };
  }
}

function readStringEnum<T extends string>(
  config: vscode.WorkspaceConfiguration,
  key: string,
  values: readonly T[],
  fallback: T,
): T {
  const value = config.get<unknown>(key, fallback);
  return typeof value === "string" && values.includes(value as T) ? (value as T) : fallback;
}

function readBoolean(
  config: vscode.WorkspaceConfiguration,
  key: string,
  fallback: boolean,
): boolean {
  const value = config.get<unknown>(key, fallback);
  return typeof value === "boolean" ? value : fallback;
}

function readString(config: vscode.WorkspaceConfiguration, key: string, fallback: string): string {
  const value = config.get<unknown>(key, fallback);
  return typeof value === "string" ? value : fallback;
}

function readStringOrNull(
  config: vscode.WorkspaceConfiguration,
  key: string,
  fallback: string | null,
): string | null {
  const value = config.get<unknown>(key, fallback);
  return typeof value === "string" || value === null ? value : fallback;
}

function readUnknownArray(
  config: vscode.WorkspaceConfiguration,
  key: string,
  fallback: unknown[] = [],
): unknown[] {
  const value = config.get<unknown>(key, fallback);
  return Array.isArray(value) ? value : fallback;
}

function readUnknownRecord(
  config: vscode.WorkspaceConfiguration,
  key: string,
): Record<string, unknown> {
  const value = config.get<unknown>(key, {});
  return isRecord(value) ? value : {};
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

function normalizeCfgAtoms(value: unknown[]): string[] {
  const atoms: string[] = [];
  const seen = new Set<string>();

  for (const item of normalizeStringArray(value)) {
    const atom = item.trim();
    if (!isCfgAtomName(atom) || seen.has(atom)) {
      continue;
    }

    atoms.push(atom);
    seen.add(atom);
  }

  return atoms;
}

function normalizeCargoOverrides(value: unknown[]): CargoOverrideConfig[] {
  const overrides: CargoOverrideConfig[] = [];

  for (const item of value) {
    if (!isRecord(item)) {
      continue;
    }

    const path = typeof item.path === "string" ? normalizeOptionalString(item.path) : undefined;
    if (path === undefined) {
      continue;
    }

    const cargoOverride: MutableCargoOverrideConfig = { path };
    const target = normalizeOverrideTarget(item.target);
    if (target !== undefined) {
      cargoOverride.target = target;
    }
    if (typeof item.allFeatures === "boolean") {
      cargoOverride.allFeatures = item.allFeatures;
    }
    if (typeof item.noDefaultFeatures === "boolean") {
      cargoOverride.noDefaultFeatures = item.noDefaultFeatures;
    }
    if (Array.isArray(item.features)) {
      cargoOverride.features = normalizeCargoFeatures(item.features);
    }

    overrides.push(cargoOverride);
  }

  return overrides;
}

function isCfgAtomName(value: string): boolean {
  return /^[A-Za-z_][A-Za-z0-9_]*$/.test(value);
}

function normalizeOverrideTarget(value: unknown): string | null | undefined {
  if (value === null) {
    return null;
  }
  if (typeof value !== "string") {
    return undefined;
  }

  return normalizeOptionalString(value) ?? null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function normalizeCargoSubcommand(value: string): string {
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : "check";
}
