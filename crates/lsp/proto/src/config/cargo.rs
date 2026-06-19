use ls_types::LSPAny;
use serde::{Deserialize, Serialize};

use super::section;

/// Protocol-level Cargo metadata target filter requested by an LSP client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CargoMetadataConfig {
    target: CargoMetadataTarget,
    all_features: bool,
    no_default_features: bool,
    features: Vec<String>,
}

impl CargoMetadataConfig {
    /// Uses an explicit target triple instead of auto-detecting the rustc host target.
    pub fn target_triple(mut self, target_triple: impl Into<String>) -> Self {
        let target_triple = target_triple.into().trim().to_string();
        self.target = if target_triple.is_empty() {
            CargoMetadataTarget::Auto
        } else {
            CargoMetadataTarget::Triple(target_triple)
        };
        self
    }

    /// Enables or disables Cargo's all-features metadata resolution mode.
    pub fn all_features(mut self, enabled: bool) -> Self {
        self.all_features = enabled;
        self
    }

    /// Enables or disables Cargo's no-default-features metadata resolution mode.
    pub fn no_default_features(mut self, enabled: bool) -> Self {
        self.no_default_features = enabled;
        self
    }

    /// Sets the explicit Cargo feature names to pass to Cargo metadata.
    pub fn custom_features(
        mut self,
        features: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.features = features
            .into_iter()
            .map(Into::into)
            .map(|feature| feature.trim().to_string())
            .filter(|feature| !feature.is_empty())
            .collect();
        self
    }

    /// Returns the configured target selection before auto-detection is resolved.
    pub fn target(&self) -> &CargoMetadataTarget {
        &self.target
    }

    /// Returns whether Cargo should enable every feature during metadata resolution.
    pub fn all_features_enabled(&self) -> bool {
        self.all_features
    }

    /// Returns whether Cargo should disable default features during metadata resolution.
    pub fn no_default_features_enabled(&self) -> bool {
        self.no_default_features
    }

    /// Returns the explicit Cargo feature names to enable during metadata resolution.
    pub fn features(&self) -> &[String] {
        &self.features
    }

    pub(super) fn from_initialization_options(options: Option<&LSPAny>) -> anyhow::Result<Self> {
        let Some(cargo) = section(options, "cargo") else {
            return Ok(Self::default());
        };

        let mut config = Self::default();

        if let Some(target) = cargo.get("target") {
            let target = target
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("rust-glancer cargo.target must be a string"))?;
            config = config.target_triple(target);
        }

        if let Some(all_features) = cargo.get("allFeatures") {
            let all_features = all_features.as_bool().ok_or_else(|| {
                anyhow::anyhow!("rust-glancer cargo.allFeatures must be a boolean")
            })?;
            config = config.all_features(all_features);
        }

        if let Some(no_default_features) = cargo.get("noDefaultFeatures") {
            let no_default_features = no_default_features.as_bool().ok_or_else(|| {
                anyhow::anyhow!("rust-glancer cargo.noDefaultFeatures must be a boolean")
            })?;
            config = config.no_default_features(no_default_features);
        }

        if let Some(features) = cargo.get("features") {
            let features = features
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("rust-glancer cargo.features must be an array"))?
                .iter()
                .enumerate()
                .map(|(idx, feature)| {
                    let feature = feature.as_str().ok_or_else(|| {
                        anyhow::anyhow!("rust-glancer cargo.features[{idx}] must be a string")
                    })?;
                    Ok(feature.to_string())
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            config = config.custom_features(features);
        }

        Ok(config)
    }
}

impl Default for CargoMetadataConfig {
    fn default() -> Self {
        Self {
            target: CargoMetadataTarget::Auto,
            all_features: false,
            no_default_features: false,
            features: Vec::new(),
        }
    }
}

/// Target platform selection for Cargo metadata filtering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CargoMetadataTarget {
    Auto,
    Triple(String),
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{CargoMetadataConfig, CargoMetadataTarget};

    #[test]
    fn parses_cargo_target() {
        let options = json!({
            "cargo": {
                "target": "x86_64-unknown-linux-gnu",
            },
        });

        let config = CargoMetadataConfig::from_initialization_options(Some(&options))
            .expect("cargo config should parse");

        assert_eq!(
            config.target(),
            &CargoMetadataTarget::Triple("x86_64-unknown-linux-gnu".to_string()),
        );
    }

    #[test]
    fn parses_cargo_feature_options() {
        let options = json!({
            "cargo": {
                "allFeatures": true,
                "noDefaultFeatures": true,
                "features": ["extra", "  tracing  ", ""],
            },
        });

        let config = CargoMetadataConfig::from_initialization_options(Some(&options))
            .expect("cargo config should parse");

        assert!(config.all_features_enabled());
        assert!(config.no_default_features_enabled());
        assert_eq!(
            config.features(),
            &["extra".to_string(), "tracing".to_string()],
        );
    }

    #[test]
    fn rejects_malformed_cargo_feature_booleans() {
        let options = json!({
            "cargo": {
                "allFeatures": "yes",
            },
        });

        let error = CargoMetadataConfig::from_initialization_options(Some(&options))
            .expect_err("malformed cargo.allFeatures should be rejected");

        assert!(
            error.to_string().contains("rust-glancer cargo.allFeatures"),
            "{error:?}",
        );
    }

    #[test]
    fn rejects_malformed_cargo_features() {
        let options = json!({
            "cargo": {
                "features": [true],
            },
        });

        let error = CargoMetadataConfig::from_initialization_options(Some(&options))
            .expect_err("malformed cargo.features should be rejected");

        assert!(
            error.to_string().contains("rust-glancer cargo.features[0]"),
            "{error:?}",
        );
    }
}
