use anyhow::Context as _;
use ls_types::LSPAny;
use serde::{Deserialize, Serialize};

use super::{
    AnalysisCfgConfig, CargoMetadataConfig, IndexingPerformancePreference, PackageBatchSize,
    PackageResidencyPolicy, SysrootDiscovery,
};

/// Analysis configuration sent by the LSP client during initialization.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AnalysisConfig {
    pub package_residency_policy: PackageResidencyPolicy,
    pub cargo_metadata_config: CargoMetadataConfig,
    #[serde(default)]
    pub sysroot_discovery: SysrootDiscovery,
    pub indexing_preference: IndexingPerformancePreference,
    pub package_batch_size: PackageBatchSize,
    pub cfg: AnalysisCfgConfig,
}

impl AnalysisConfig {
    pub fn from_initialization_options(options: Option<&LSPAny>) -> anyhow::Result<Self> {
        Ok(Self {
            package_residency_policy: PackageResidencyPolicy::from_initialization_options(options),
            cargo_metadata_config: CargoMetadataConfig::from_initialization_options(options)
                .context("parse Cargo metadata configuration")?,
            sysroot_discovery: SysrootDiscovery::from_initialization_options(options)
                .context("parse sysroot discovery configuration")?,
            indexing_preference: IndexingPerformancePreference::from_initialization_options(
                options,
            )
            .context("parse indexing performance preference")?,
            package_batch_size: PackageBatchSize::from_initialization_options(options)
                .context("parse package batch size")?,
            cfg: AnalysisCfgConfig::from_initialization_options(options)
                .context("parse cfg configuration")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AnalysisCfgConfig, AnalysisConfig, CargoMetadataConfig, IndexingPerformancePreference,
        PackageBatchSize, PackageResidencyPolicy, SysrootDiscovery,
    };

    #[test]
    fn defaults_to_all_offloadable_residency() {
        let config = AnalysisConfig::from_initialization_options(None)
            .expect("default analysis config should parse");

        assert_eq!(config, AnalysisConfig::default());
        assert_eq!(
            config.package_residency_policy,
            PackageResidencyPolicy::AllOffloadable,
        );
        assert_eq!(config.cargo_metadata_config, CargoMetadataConfig::default(),);
        assert_eq!(config.sysroot_discovery, SysrootDiscovery::Auto);
        assert_eq!(
            config.indexing_preference,
            IndexingPerformancePreference::FasterBuilds,
        );
        assert_eq!(config.package_batch_size, PackageBatchSize::default());
        assert_eq!(config.cfg, AnalysisCfgConfig::default());
    }
}
