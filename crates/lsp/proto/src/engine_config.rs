use ls_types::LSPAny;
use serde::{Deserialize, Serialize};

use crate::{AnalysisConfig, DiagnosticsConfig};

/// Configuration needed to start one analysis engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EngineConfig {
    pub analysis: AnalysisConfig,
    pub diagnostics: DiagnosticsConfig,
}

impl EngineConfig {
    pub fn from_initialization_options(options: Option<&LSPAny>) -> anyhow::Result<Self> {
        Ok(Self {
            analysis: AnalysisConfig::from_initialization_options(options),
            diagnostics: DiagnosticsConfig::from_initialization_options(options)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use ls_types::LSPAny;
    use rg_project::PackageResidencyPolicy;
    use rg_workspace::CargoMetadataTarget;

    use super::EngineConfig;

    #[test]
    fn parses_engine_configuration() {
        let options = object([
            (
                "cache",
                object([(
                    "packageResidency",
                    LSPAny::String("workspace-resident".to_string()),
                )]),
            ),
            (
                "cargo",
                object([(
                    "target",
                    LSPAny::String("x86_64-unknown-linux-gnu".to_string()),
                )]),
            ),
            (
                "diagnostics",
                object([
                    ("onStartup", LSPAny::Bool(true)),
                    ("command", LSPAny::String("clippy".to_string())),
                ]),
            ),
        ]);

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
        assert!(config.diagnostics.on_startup);
        assert_eq!(config.diagnostics.command, "clippy");
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
