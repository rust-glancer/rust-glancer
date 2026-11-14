use anyhow::Context as _;
use gen_lsp_types::LspAny;
use serde::{Deserialize, Serialize};

use super::{AnalysisCfgConfig, CargoMetadataConfig, SysrootDiscovery, section};

/// Analysis configuration sent by the LSP client during initialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisConfig {
    pub cargo_metadata_config: CargoMetadataConfig,
    #[serde(default)]
    pub sysroot_discovery: SysrootDiscovery,
    pub cfg: AnalysisCfgConfig,
    /// Reuse query results from earlier processes. Disabling this clears the opened analysis
    /// cache before indexing; results computed by this process are still cached normally.
    ///
    /// This is an internal option for debugging / benchmarking, not a part of public API.
    pub reuse_cache: bool,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            cargo_metadata_config: CargoMetadataConfig::default(),
            sysroot_discovery: SysrootDiscovery::default(),
            cfg: AnalysisCfgConfig::default(),
            reuse_cache: true,
        }
    }
}

impl AnalysisConfig {
    pub fn from_initialization_options(options: Option<&LspAny>) -> anyhow::Result<Self> {
        let mut config = Self {
            cargo_metadata_config: CargoMetadataConfig::from_initialization_options(options)
                .context("parse Cargo metadata configuration")?,
            sysroot_discovery: SysrootDiscovery::from_initialization_options(options)
                .context("parse sysroot discovery configuration")?,
            cfg: AnalysisCfgConfig::from_initialization_options(options)
                .context("parse cfg configuration")?,
            ..Self::default()
        };
        if let Some(value) = section(options, "cache").and_then(|cache| cache.get("reuseOnStartup"))
        {
            config.reuse_cache = value.as_bool().ok_or_else(|| {
                anyhow::anyhow!("rust-glancer cache.reuseOnStartup must be a boolean")
            })?;
        }
        Ok(config)
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
