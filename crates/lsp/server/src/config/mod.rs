mod cargo_overrides;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use rg_lsp_proto::EngineConfig;
use tower_lsp_server::ls_types::LSPAny;

use self::cargo_overrides::CargoConfigOverrides;

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
