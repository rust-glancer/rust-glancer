mod analysis;
mod cache;
mod cargo;
mod cfg;
mod diagnostics;
mod indexing;

use ls_types::LSPAny;
use serde::{Deserialize, Serialize};

pub use self::{
    analysis::AnalysisConfig,
    cache::PackageResidencyPolicy,
    cargo::{CargoMetadataConfig, CargoMetadataTarget},
    cfg::AnalysisCfgConfig,
    diagnostics::DiagnosticsConfig,
    indexing::IndexingPerformancePreference,
};

/// Configuration needed to start one analysis engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EngineConfig {
    pub analysis: AnalysisConfig,
    pub diagnostics: DiagnosticsConfig,
}

impl EngineConfig {
    pub fn from_initialization_options(options: Option<&LSPAny>) -> anyhow::Result<Self> {
        Ok(Self {
            analysis: AnalysisConfig::from_initialization_options(options)?,
            diagnostics: DiagnosticsConfig::from_initialization_options(options)?,
        })
    }
}

fn section<'a>(options: Option<&'a LSPAny>, key: &'static str) -> Option<&'a ls_types::LSPObject> {
    options
        .and_then(LSPAny::as_object)
        .and_then(|options| options.get(key))
        .and_then(LSPAny::as_object)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        CargoMetadataTarget, EngineConfig, IndexingPerformancePreference, PackageResidencyPolicy,
    };

    #[test]
    fn parses_engine_configuration() {
        let options = json!({
            "cache": {
                "packageResidency": "workspace-resident",
            },
            "cargo": {
                "target": "x86_64-unknown-linux-gnu",
                "allFeatures": true,
                "noDefaultFeatures": true,
                "features": ["serde", "derive"],
            },
            "indexing": {
                "performancePreference": "faster-builds",
            },
            "cfg": {
                "test": true,
                "atoms": ["tokio_unstable"],
            },
            "diagnostics": {
                "onStartup": true,
                "command": "clippy",
            },
        });

        let config = EngineConfig::from_initialization_options(Some(&options))
            .expect("engine config should parse");

        assert_eq!(
            config.analysis.package_residency_policy,
            PackageResidencyPolicy::WorkspaceResident,
        );
        assert_eq!(
            config.analysis.cargo_metadata_config.target(),
            &CargoMetadataTarget::Triple("x86_64-unknown-linux-gnu".to_string()),
        );
        assert!(config.analysis.cargo_metadata_config.all_features_enabled());
        assert!(
            config
                .analysis
                .cargo_metadata_config
                .no_default_features_enabled()
        );
        assert_eq!(
            config.analysis.cargo_metadata_config.features(),
            &["serde".to_string(), "derive".to_string()],
        );
        assert_eq!(
            config.analysis.indexing_preference,
            IndexingPerformancePreference::FasterBuilds,
        );
        assert!(config.analysis.cfg.test);
        assert_eq!(config.analysis.cfg.atoms, ["tokio_unstable"]);
        assert!(config.diagnostics.on_startup);
        assert_eq!(config.diagnostics.command, "clippy");
    }
}
