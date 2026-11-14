//! Translation from client configuration into project-domain configuration.

use rg_lsp_proto::{
    AnalysisConfig, CargoMetadataTarget as ProtoCargoMetadataTarget,
    SysrootDiscovery as ProtoSysrootDiscovery,
};
#[cfg(test)]
use rg_workspace::SysrootSources;
use rg_workspace::{CargoMetadataConfig, WorkspaceLoweringConfig};

/// Project settings after client-facing enum and option shapes have been removed.
#[derive(Debug, Clone)]
pub(crate) struct ProjectConfiguration {
    pub(super) cargo_metadata_config: CargoMetadataConfig,
    pub(super) workspace_lowering_config: WorkspaceLoweringConfig,
    pub(super) discover_sysroot: bool,
    pub(super) reuse_cache: bool,
    #[cfg(test)]
    pub(crate) sysroot_sources: Option<SysrootSources>,
}

impl From<AnalysisConfig> for ProjectConfiguration {
    /// Remove protocol-only enum shapes before project construction reaches the engine lane.
    fn from(config: AnalysisConfig) -> Self {
        let cargo_metadata_config = match config.cargo_metadata_config.target() {
            ProtoCargoMetadataTarget::Auto => CargoMetadataConfig::default(),
            ProtoCargoMetadataTarget::Triple(target) => {
                CargoMetadataConfig::default().target_triple(target.as_str())
            }
        }
        .all_features(config.cargo_metadata_config.all_features_enabled())
        .no_default_features(config.cargo_metadata_config.no_default_features_enabled())
        .custom_features(config.cargo_metadata_config.features().iter().cloned());
        let workspace_lowering_config = WorkspaceLoweringConfig::default()
            .cfg_test(config.cfg.test)
            .custom_cfg_atoms(config.cfg.atoms);

        Self {
            cargo_metadata_config,
            workspace_lowering_config,
            discover_sysroot: matches!(config.sysroot_discovery, ProtoSysrootDiscovery::Auto),
            reuse_cache: config.reuse_cache,
            #[cfg(test)]
            sysroot_sources: None,
        }
    }
}
