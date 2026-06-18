use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use rg_lsp_proto::{CargoMetadataConfig, CargoMetadataTarget};
use tower_lsp_server::ls_types::{LSPAny, LSPObject};

/// Path-indexed Cargo metadata overrides for workspace engine startup.
///
/// Overrides are selected by exact normalized Cargo workspace root. Relative paths from client
/// settings are expanded against the VS Code workspace folders before they enter this table, so
/// resolving an engine config is just a root lookup.
#[derive(Debug, Clone, Default)]
pub(super) struct CargoConfigOverrides {
    by_root: BTreeMap<PathBuf, CargoConfigOverride>,
}

impl CargoConfigOverrides {
    pub(super) fn from_initialization_options(
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

    pub(super) fn override_for_root(&self, root: &Path) -> Option<&CargoConfigOverride> {
        self.by_root.get(&normalize_path(root))
    }
}

/// Partial replacement for the Cargo metadata part of an engine config.
///
/// Each field is optional so an override can mention only the Cargo settings that differ for one
/// workspace root. Missing fields inherit from the top-level Cargo config, while explicit values
/// such as `features: []` or `target: null` replace the inherited value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CargoConfigOverride {
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

    pub(super) fn apply_to(&self, mut config: CargoMetadataConfig) -> CargoMetadataConfig {
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
