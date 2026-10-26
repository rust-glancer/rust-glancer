use anyhow::Context as _;
use gen_lsp_types::LspAny;
use serde::{Deserialize, Serialize};

use super::{
    AnalysisCfgConfig, CargoMetadataConfig, IndexingPerformancePreference, PackageBatchSize,
    SysrootDiscovery,
};

/// Analysis configuration sent by the LSP client during initialization.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AnalysisConfig {
    pub cargo_metadata_config: CargoMetadataConfig,
    #[serde(default)]
    pub sysroot_discovery: SysrootDiscovery,
    pub indexing_preference: IndexingPerformancePreference,
    pub package_batch_size: PackageBatchSize,
    pub cfg: AnalysisCfgConfig,
}

impl AnalysisConfig {
    pub fn from_initialization_options(options: Option<&LspAny>) -> anyhow::Result<Self> {
        Ok(Self {
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
    use super::AnalysisConfig;

    #[test]
    fn parses_default_analysis_config() {
        assert_eq!(
            AnalysisConfig::from_initialization_options(None)
                .expect("default analysis config should parse"),
            AnalysisConfig::default(),
        );
    }
}
