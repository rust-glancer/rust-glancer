use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use rg_lsp_proto::{CargoMetadataConfig, CargoMetadataTarget, EngineConfig};
use tower_lsp_server::ls_types::{LSPAny, LSPObject};

/// Server-local configuration for resolving per-engine settings.
///
/// The engine protocol deliberately receives a concrete `EngineConfig` per Cargo workspace. The
/// server owns path routing, so it also owns path-specific override selection before an engine is
/// spawned.
#[derive(Debug, Clone)]
pub(crate) struct ServerConfig {
    engine_config: EngineConfig,
    cargo_overrides: CargoConfigOverrides,
}

impl ServerConfig {
    pub(crate) fn from_initialization_options(
        options: Option<&LSPAny>,
        workspace_folders: &[PathBuf],
    ) -> anyhow::Result<Self> {
        Ok(Self {
            engine_config: EngineConfig::from_initialization_options(options)?,
            cargo_overrides: CargoConfigOverrides::from_initialization_options(
                options,
                workspace_folders,
            )?,
        })
    }

    #[cfg(test)]
    pub(crate) fn from_engine_config(engine_config: EngineConfig) -> Self {
        Self {
            engine_config,
            cargo_overrides: CargoConfigOverrides::default(),
        }
    }

    pub(crate) fn engine_config_for_root(&self, root: &Path) -> EngineConfig {
        let mut config = self.engine_config.clone();
        if let Some(cargo_override) = self.cargo_overrides.override_for_root(root) {
            config.analysis.cargo_metadata_config =
                cargo_override.apply_to(config.analysis.cargo_metadata_config);
        }
        config
    }
}

/// Path-indexed Cargo metadata overrides for workspace engine startup.
///
/// Overrides are selected by exact normalized Cargo workspace root. Relative paths from client
/// settings are expanded against the VS Code workspace folders before they enter this table, so
/// resolving an engine config is just a root lookup.
#[derive(Debug, Clone, Default)]
struct CargoConfigOverrides {
    by_root: BTreeMap<PathBuf, CargoConfigOverride>,
}

impl CargoConfigOverrides {
    fn from_initialization_options(
        options: Option<&LSPAny>,
        workspace_folders: &[PathBuf],
    ) -> anyhow::Result<Self> {
        let Some(overrides) = options
            .and_then(LSPAny::as_object)
            .and_then(|options| options.get("cargo"))
            .and_then(LSPAny::as_object)
            .and_then(|cargo| cargo.get("overrides"))
        else {
            return Ok(Self::default());
        };

        let overrides = overrides
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("rust-glancer cargo.overrides must be an array"))?;
        let mut by_root = BTreeMap::new();

        for (idx, item) in overrides.iter().enumerate() {
            let item = item.as_object().ok_or_else(|| {
                anyhow::anyhow!("rust-glancer cargo.overrides[{idx}] must be an object")
            })?;
            let path = item
                .get("path")
                .and_then(LSPAny::as_str)
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "rust-glancer cargo.overrides[{idx}].path must be a non-empty string"
                    )
                })?;
            let cargo_override = CargoConfigOverride::parse(item, idx)?;

            for root in override_roots(path, workspace_folders) {
                by_root.insert(root, cargo_override.clone());
            }
        }

        Ok(Self { by_root })
    }

    fn override_for_root(&self, root: &Path) -> Option<&CargoConfigOverride> {
        self.by_root.get(&normalize_path(root))
    }
}

/// Partial replacement for the Cargo metadata part of an engine config.
///
/// Each field is optional so an override can mention only the Cargo settings that differ for one
/// workspace root. Missing fields inherit from the top-level Cargo config, while explicit values
/// such as `features: []` or `target: null` replace the inherited value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CargoConfigOverride {
    target: Option<CargoMetadataTarget>,
    all_features: Option<bool>,
    no_default_features: Option<bool>,
    features: Option<Vec<String>>,
}

impl CargoConfigOverride {
    fn parse(item: &LSPObject, idx: usize) -> anyhow::Result<CargoConfigOverride> {
        Ok(Self {
            target: Self::parse_target(item, idx)?,
            all_features: Self::parse_bool(item, idx, "allFeatures")?,
            no_default_features: Self::parse_bool(item, idx, "noDefaultFeatures")?,
            features: Self::parse_features(item, idx)?,
        })
    }

    fn parse_target(item: &LSPObject, idx: usize) -> anyhow::Result<Option<CargoMetadataTarget>> {
        let Some(value) = item.get("target") else {
            return Ok(None);
        };
        if value.is_null() {
            return Ok(Some(CargoMetadataTarget::Auto));
        }

        let target = value.as_str().ok_or_else(|| {
            anyhow::anyhow!("rust-glancer cargo.overrides[{idx}].target must be a string or null")
        })?;
        let target = target.trim();
        if target.is_empty() {
            Ok(Some(CargoMetadataTarget::Auto))
        } else {
            Ok(Some(CargoMetadataTarget::Triple(target.to_string())))
        }
    }

    fn parse_bool(item: &LSPObject, idx: usize, key: &'static str) -> anyhow::Result<Option<bool>> {
        let Some(value) = item.get(key) else {
            return Ok(None);
        };
        value.as_bool().map(Some).ok_or_else(|| {
            anyhow::anyhow!("rust-glancer cargo.overrides[{idx}].{key} must be a boolean")
        })
    }

    fn parse_features(item: &LSPObject, idx: usize) -> anyhow::Result<Option<Vec<String>>> {
        let Some(value) = item.get("features") else {
            return Ok(None);
        };

        let features = value.as_array().ok_or_else(|| {
            anyhow::anyhow!("rust-glancer cargo.overrides[{idx}].features must be an array")
        })?;
        features
            .iter()
            .enumerate()
            .map(|(feature_idx, feature)| {
                let feature = feature.as_str().ok_or_else(|| {
                    anyhow::anyhow!(
                        "rust-glancer cargo.overrides[{idx}].features[{feature_idx}] must be a string"
                    )
                })?;
                Ok(feature.to_string())
            })
            .collect::<anyhow::Result<Vec<_>>>()
            .map(Some)
    }

    fn apply_to(&self, mut config: CargoMetadataConfig) -> CargoMetadataConfig {
        if let Some(target) = &self.target {
            config = match target {
                CargoMetadataTarget::Auto => config.target_triple(""),
                CargoMetadataTarget::Triple(target) => config.target_triple(target.as_str()),
            };
        }
        if let Some(all_features) = self.all_features {
            config = config.all_features(all_features);
        }
        if let Some(no_default_features) = self.no_default_features {
            config = config.no_default_features(no_default_features);
        }
        if let Some(features) = &self.features {
            config = config.custom_features(features.iter().cloned());
        }

        config
    }
}

fn override_roots(path: &str, workspace_folders: &[PathBuf]) -> Vec<PathBuf> {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        return vec![normalize_path(path)];
    }

    workspace_folders
        .iter()
        .map(|workspace_folder| normalize_path(workspace_folder.join(&path)))
        .collect()
}

fn normalize_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use rg_lsp_proto::{CargoMetadataTarget, PackageResidencyPolicy};
    use serde_json::json;

    use super::*;

    #[test]
    fn exact_override_merges_with_base_cargo_config() {
        let options = json!({
            "cache": {
                "packageResidency": "all-resident",
            },
            "cargo": {
                "target": "x86_64-unknown-linux-gnu",
                "allFeatures": true,
                "noDefaultFeatures": false,
                "features": ["base"],
                "overrides": [{
                    "path": "project-a",
                    "noDefaultFeatures": true,
                    "features": [],
                }],
            },
        });

        let config =
            ServerConfig::from_initialization_options(Some(&options), &[PathBuf::from("/repo")])
                .expect("server config should parse");
        let project_config = config.engine_config_for_root(Path::new("/repo/project-a"));
        let default_config = config.engine_config_for_root(Path::new("/repo/project-b"));

        assert_eq!(
            project_config.analysis.package_residency_policy,
            PackageResidencyPolicy::AllResident,
            "non-cargo engine settings should remain inherited",
        );
        assert_eq!(
            project_config.analysis.cargo_metadata_config.target(),
            &CargoMetadataTarget::Triple("x86_64-unknown-linux-gnu".to_string()),
        );
        assert!(
            project_config
                .analysis
                .cargo_metadata_config
                .all_features_enabled()
        );
        assert!(
            project_config
                .analysis
                .cargo_metadata_config
                .no_default_features_enabled()
        );
        assert!(
            project_config
                .analysis
                .cargo_metadata_config
                .features()
                .is_empty(),
            "explicit empty features should clear inherited custom features",
        );

        assert!(
            !default_config
                .analysis
                .cargo_metadata_config
                .no_default_features_enabled()
        );
        assert_eq!(
            default_config.analysis.cargo_metadata_config.features(),
            &["base".to_string()],
        );
    }

    #[test]
    fn override_matches_exact_engine_root_only() {
        let options = json!({
            "cargo": {
                "features": ["base"],
                "overrides": [{
                    "path": "project-a",
                    "features": ["override"],
                }],
            },
        });
        let config =
            ServerConfig::from_initialization_options(Some(&options), &[PathBuf::from("/repo")])
                .expect("server config should parse");

        assert_eq!(
            config
                .engine_config_for_root(Path::new("/repo/project-a"))
                .analysis
                .cargo_metadata_config
                .features(),
            &["override".to_string()],
        );
        assert_eq!(
            config
                .engine_config_for_root(Path::new("/repo/project-a/nested"))
                .analysis
                .cargo_metadata_config
                .features(),
            &["base".to_string()],
            "child workspace roots should not inherit a parent override",
        );
        assert_eq!(
            config
                .engine_config_for_root(Path::new("/repo"))
                .analysis
                .cargo_metadata_config
                .features(),
            &["base".to_string()],
            "parent workspace roots should not inherit a child override",
        );
    }

    #[test]
    fn latest_duplicate_override_wins_without_warning() {
        let options = json!({
            "cargo": {
                "overrides": [
                    {
                        "path": "/repo/project",
                        "allFeatures": true,
                        "features": ["old"],
                    },
                    {
                        "path": "/repo/project",
                        "allFeatures": false,
                        "features": ["new"],
                    },
                ],
            },
        });
        let config = ServerConfig::from_initialization_options(Some(&options), &[])
            .expect("server config should parse");
        let project_config = config.engine_config_for_root(Path::new("/repo/project"));

        assert!(
            !project_config
                .analysis
                .cargo_metadata_config
                .all_features_enabled()
        );
        assert_eq!(
            project_config.analysis.cargo_metadata_config.features(),
            &["new".to_string()],
        );
    }

    #[test]
    fn override_target_null_resets_inherited_target() {
        let options = json!({
            "cargo": {
                "target": "x86_64-unknown-linux-gnu",
                "overrides": [{
                    "path": "project-a",
                    "target": null,
                }],
            },
        });
        let config =
            ServerConfig::from_initialization_options(Some(&options), &[PathBuf::from("/repo")])
                .expect("server config should parse");

        assert_eq!(
            config
                .engine_config_for_root(Path::new("/repo/project-a"))
                .analysis
                .cargo_metadata_config
                .target(),
            &CargoMetadataTarget::Auto,
        );
    }

    #[test]
    fn rejects_malformed_override_entries() {
        let options = json!({
            "cargo": {
                "overrides": [{
                    "path": "project-a",
                    "features": [true],
                }],
            },
        });
        let error =
            ServerConfig::from_initialization_options(Some(&options), &[PathBuf::from("/repo")])
                .expect_err("malformed override feature entries should be rejected");

        assert!(
            error
                .to_string()
                .contains("rust-glancer cargo.overrides[0].features[0]"),
            "{error:?}",
        );
    }
}
