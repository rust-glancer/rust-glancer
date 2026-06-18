use ls_types::LSPAny;
use serde::{Deserialize, Serialize};

use super::{
    AnalysisCfgConfig, CargoMetadataConfig, IndexingPerformancePreference, PackageResidencyPolicy,
};

/// Analysis configuration sent by the LSP client during initialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisConfig {
    pub package_residency_policy: PackageResidencyPolicy,
    pub cargo_metadata_config: CargoMetadataConfig,
    pub indexing_preference: IndexingPerformancePreference,
    pub cfg: AnalysisCfgConfig,
}

impl AnalysisConfig {
    pub fn from_initialization_options(options: Option<&LSPAny>) -> anyhow::Result<Self> {
        Ok(Self {
            package_residency_policy: PackageResidencyPolicy::from_initialization_options(options),
            cargo_metadata_config: CargoMetadataConfig::from_initialization_options(options)?,
            indexing_preference: IndexingPerformancePreference::from_initialization_options(
                options,
            )?,
            cfg: AnalysisCfgConfig::from_initialization_options(options)?,
        })
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
    use super::{
        AnalysisCfgConfig, AnalysisConfig, CargoMetadataConfig, IndexingPerformancePreference,
        PackageResidencyPolicy,
    };

    #[test]
    fn defaults_to_workspace_and_path_dependency_residency() {
        let config = AnalysisConfig::from_initialization_options(None)
            .expect("default analysis config should parse");

        assert_eq!(
            config.package_residency_policy,
            PackageResidencyPolicy::WorkspaceAndPathDepsResident,
        );
        assert_eq!(config.cargo_metadata_config, CargoMetadataConfig::default(),);
        assert_eq!(
            config.indexing_preference,
            IndexingPerformancePreference::FasterBuilds,
        );
        assert_eq!(config.cfg, AnalysisCfgConfig::default());
    }
}
