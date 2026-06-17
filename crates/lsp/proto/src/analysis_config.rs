use ls_types::LSPAny;
use serde::{Deserialize, Serialize};

/// Analysis configuration sent by the LSP client during initialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisConfig {
    pub package_residency_policy: PackageResidencyPolicy,
    pub cargo_metadata_config: CargoMetadataConfig,
    pub indexing_preference: IndexingPerformancePreference,
    pub cfg: AnalysisCfgConfig,
}

/// Protocol-level cfg atoms requested by an LSP client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AnalysisCfgConfig {
    pub test: bool,
}

/// Protocol-level cache residency policy requested by an LSP client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PackageResidencyPolicy {
    AllResident,
    WorkspaceResident,
    WorkspaceAndPathDepsResident,
    WorkspacePathAndDirectDepsResident,
    AllOffloadable,
}

impl PackageResidencyPolicy {
    /// Stable kebab-case name accepted in LSP initialization options.
    pub fn config_name(self) -> &'static str {
        match self {
            Self::AllResident => "all-resident",
            Self::WorkspaceResident => "workspace",
            Self::WorkspaceAndPathDepsResident => "workspace-and-path-deps",
            Self::WorkspacePathAndDirectDepsResident => "workspace-path-and-direct-deps",
            Self::AllOffloadable => "all-offloadable",
        }
    }

    /// Parses the public policy names accepted by frontends.
    pub fn from_config_name(value: &str) -> Option<Self> {
        let normalized = value.trim().replace('_', "-").to_ascii_lowercase();
        match normalized.as_str() {
            "all-resident" => Some(Self::AllResident),
            "workspace" | "workspace-resident" => Some(Self::WorkspaceResident),
            "workspace-and-path-deps" | "workspace-path-deps" => {
                Some(Self::WorkspaceAndPathDepsResident)
            }
            "workspace-path-and-direct-deps" | "workspace-path-direct-deps" => {
                Some(Self::WorkspacePathAndDirectDepsResident)
            }
            "all-offloadable" => Some(Self::AllOffloadable),
            _ => None,
        }
    }
}

/// Protocol-level indexing trade-off requested by an LSP client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum IndexingPerformancePreference {
    LowerPeakMemory,
    #[default]
    FasterBuilds,
}

impl IndexingPerformancePreference {
    /// Stable kebab-case name accepted in LSP initialization options.
    pub fn config_name(self) -> &'static str {
        match self {
            Self::LowerPeakMemory => "lower-peak-memory",
            Self::FasterBuilds => "faster-builds",
        }
    }

    /// Parses the public preference names accepted by frontends.
    pub fn from_config_name(value: &str) -> Option<Self> {
        match value {
            "lower-peak-memory" => Some(Self::LowerPeakMemory),
            "faster-builds" => Some(Self::FasterBuilds),
            _ => None,
        }
    }
}

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

    fn from_initialization_options(options: Option<&LSPAny>) -> anyhow::Result<Self> {
        let Some(cargo) = options
            .and_then(LSPAny::as_object)
            .and_then(|options| options.get("cargo"))
            .and_then(LSPAny::as_object)
        else {
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

impl AnalysisConfig {
    pub fn from_initialization_options(options: Option<&LSPAny>) -> anyhow::Result<Self> {
        let default = Self::default();
        let cfg = AnalysisCfgConfig::from_initialization_options(options)?;
        let package_residency_policy = options
            .and_then(LSPAny::as_object)
            .and_then(|options| {
                options
                    .get("cache")
                    .and_then(LSPAny::as_object)
                    .and_then(|cache| cache.get("packageResidency"))
            })
            .and_then(LSPAny::as_str)
            .and_then(PackageResidencyPolicy::from_config_name)
            .unwrap_or(default.package_residency_policy);
        let cargo_metadata_config = CargoMetadataConfig::from_initialization_options(options)?;
        let indexing_preference = match options.and_then(LSPAny::as_object).and_then(|options| {
            options
                .get("indexing")
                .and_then(LSPAny::as_object)
                .and_then(|indexing| indexing.get("performancePreference"))
        }) {
            Some(value) => {
                let value = value.as_str().ok_or_else(|| {
                    anyhow::anyhow!("rust-glancer indexing.performancePreference must be a string")
                })?;
                IndexingPerformancePreference::from_config_name(value).ok_or_else(|| {
                    anyhow::anyhow!(
                        "rust-glancer indexing.performancePreference must be one of: lower-peak-memory, faster-builds"
                    )
                })?
            }
            None => default.indexing_preference,
        };

        Ok(Self {
            package_residency_policy,
            cargo_metadata_config,
            indexing_preference,
            cfg,
        })
    }
}

impl AnalysisCfgConfig {
    pub fn from_initialization_options(options: Option<&LSPAny>) -> anyhow::Result<Self> {
        let Some(cfg) = options
            .and_then(LSPAny::as_object)
            .and_then(|options| options.get("cfg"))
            .and_then(LSPAny::as_object)
        else {
            return Ok(Self::default());
        };

        let test = match cfg.get("test") {
            Some(value) => value
                .as_bool()
                .ok_or_else(|| anyhow::anyhow!("rust-glancer cfg.test must be a boolean"))?,
            None => false,
        };

        Ok(Self { test })
    }
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            // LSP keeps the packages users are most likely to edit resident, but otherwise favors
            // fast initial indexing unless a client explicitly asks to lower peak memory.
            package_residency_policy: PackageResidencyPolicy::WorkspaceAndPathDepsResident,
            cargo_metadata_config: CargoMetadataConfig::default(),
            indexing_preference: IndexingPerformancePreference::default(),
            cfg: AnalysisCfgConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use ls_types::LSPAny;

    use super::{
        AnalysisConfig, CargoMetadataTarget, IndexingPerformancePreference, PackageResidencyPolicy,
    };

    #[test]
    fn defaults_to_workspace_and_path_dependency_residency() {
        let config = AnalysisConfig::from_initialization_options(None)
            .expect("default analysis config should parse");

        assert_eq!(
            config.package_residency_policy,
            PackageResidencyPolicy::WorkspaceAndPathDepsResident,
        );
        assert_eq!(
            config.cargo_metadata_config.target(),
            &CargoMetadataTarget::Auto
        );
        assert!(!config.cargo_metadata_config.all_features_enabled());
        assert!(!config.cargo_metadata_config.no_default_features_enabled());
        assert!(config.cargo_metadata_config.features().is_empty());
        assert_eq!(
            config.indexing_preference,
            IndexingPerformancePreference::FasterBuilds,
        );
        assert!(!config.cfg.test);
    }

    #[test]
    fn parses_cache_residency_policy() {
        let options = object([(
            "cache",
            object([(
                "packageResidency",
                LSPAny::String("all-resident".to_string()),
            )]),
        )]);

        let config = AnalysisConfig::from_initialization_options(Some(&options))
            .expect("analysis config should parse");

        assert_eq!(
            config.package_residency_policy,
            PackageResidencyPolicy::AllResident,
        );
    }

    #[test]
    fn parses_cargo_target() {
        let options = object([(
            "cargo",
            object([(
                "target",
                LSPAny::String("x86_64-unknown-linux-gnu".to_string()),
            )]),
        )]);

        let config = AnalysisConfig::from_initialization_options(Some(&options))
            .expect("analysis config should parse");

        assert_eq!(
            config.cargo_metadata_config.target(),
            &CargoMetadataTarget::Triple("x86_64-unknown-linux-gnu".to_string()),
        );
    }

    #[test]
    fn parses_cargo_feature_options() {
        let options = object([(
            "cargo",
            object([
                ("allFeatures", LSPAny::Bool(true)),
                ("noDefaultFeatures", LSPAny::Bool(true)),
                (
                    "features",
                    LSPAny::Array(vec![
                        LSPAny::String("extra".to_string()),
                        LSPAny::String("  tracing  ".to_string()),
                        LSPAny::String(String::new()),
                    ]),
                ),
            ]),
        )]);

        let config = AnalysisConfig::from_initialization_options(Some(&options))
            .expect("analysis config should parse");

        assert!(config.cargo_metadata_config.all_features_enabled());
        assert!(config.cargo_metadata_config.no_default_features_enabled());
        assert_eq!(
            config.cargo_metadata_config.features(),
            &["extra".to_string(), "tracing".to_string()],
        );
    }

    #[test]
    fn parses_indexing_preference() {
        let options = object([(
            "indexing",
            object([(
                "performancePreference",
                LSPAny::String("faster-builds".to_string()),
            )]),
        )]);

        let config = AnalysisConfig::from_initialization_options(Some(&options))
            .expect("analysis config should parse");

        assert_eq!(
            config.indexing_preference,
            IndexingPerformancePreference::FasterBuilds,
        );
    }

    #[test]
    fn parses_cfg_test() {
        let options = object([("cfg", object([("test", LSPAny::Bool(true))]))]);

        let config = AnalysisConfig::from_initialization_options(Some(&options))
            .expect("analysis config should parse");

        assert!(config.cfg.test);
    }

    #[test]
    fn rejects_unknown_indexing_preference() {
        let options = object([(
            "indexing",
            object([("performancePreference", LSPAny::String("fast".to_string()))]),
        )]);

        let error = AnalysisConfig::from_initialization_options(Some(&options))
            .expect_err("unknown indexing preference should be rejected");

        assert!(
            error
                .to_string()
                .contains("rust-glancer indexing.performancePreference"),
            "{error:?}",
        );
    }

    #[test]
    fn rejects_malformed_cfg_test() {
        let options = object([("cfg", object([("test", LSPAny::String("yes".to_string()))]))]);

        let error = AnalysisConfig::from_initialization_options(Some(&options))
            .expect_err("malformed cfg.test should be rejected");

        assert!(
            error.to_string().contains("rust-glancer cfg.test"),
            "{error:?}",
        );
    }

    #[test]
    fn rejects_malformed_cargo_feature_booleans() {
        let options = object([(
            "cargo",
            object([("allFeatures", LSPAny::String("yes".to_string()))]),
        )]);

        let error = AnalysisConfig::from_initialization_options(Some(&options))
            .expect_err("malformed cargo.allFeatures should be rejected");

        assert!(
            error.to_string().contains("rust-glancer cargo.allFeatures"),
            "{error:?}",
        );
    }

    #[test]
    fn rejects_malformed_cargo_features() {
        let options = object([(
            "cargo",
            object([("features", LSPAny::Array(vec![LSPAny::Bool(true)]))]),
        )]);

        let error = AnalysisConfig::from_initialization_options(Some(&options))
            .expect_err("malformed cargo.features should be rejected");

        assert!(
            error.to_string().contains("rust-glancer cargo.features[0]"),
            "{error:?}",
        );
    }

    fn object<const N: usize>(entries: [(&str, LSPAny); N]) -> LSPAny {
        let mut map = match LSPAny::Object(Default::default()) {
            LSPAny::Object(map) => map,
            _ => unreachable!("constructed object should be an object"),
        };
        for (key, value) in entries {
            map.insert(key.to_string(), value);
        }
        LSPAny::Object(map)
    }
}
