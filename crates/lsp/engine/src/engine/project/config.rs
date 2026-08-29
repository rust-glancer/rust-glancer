//! Translation from client configuration into project-domain configuration.

use rg_lsp_proto::{
    AnalysisConfig, CargoMetadataTarget as ProtoCargoMetadataTarget,
    IndexingPerformancePreference as ProtoIndexingPerformancePreference,
    PackageResidencyPolicy as ProtoPackageResidencyPolicy,
    SysrootDiscovery as ProtoSysrootDiscovery,
};
use rg_project::{IndexingPerformancePreference, PackageBatchSize, PackageResidencyPolicy};
use rg_workspace::{CargoMetadataConfig, WorkspaceLoweringConfig};

/// Project settings after client-facing enum and option shapes have been removed.
#[derive(Debug, Clone)]
pub(crate) struct ProjectConfiguration {
    pub(super) package_residency_policy: PackageResidencyPolicy,
    pub(super) cargo_metadata_config: CargoMetadataConfig,
    pub(super) workspace_lowering_config: WorkspaceLoweringConfig,
    pub(super) indexing_preference: IndexingPerformancePreference,
    pub(super) package_batch_size: PackageBatchSize,
    pub(super) discover_sysroot: bool,
}

impl From<AnalysisConfig> for ProjectConfiguration {
    /// Remove protocol-only enum shapes before project construction reaches the engine lane.
    fn from(config: AnalysisConfig) -> Self {
        let package_residency_policy = match config.package_residency_policy {
            ProtoPackageResidencyPolicy::AllResident => PackageResidencyPolicy::AllResident,
            ProtoPackageResidencyPolicy::WorkspaceResident => {
                PackageResidencyPolicy::WorkspaceResident
            }
            ProtoPackageResidencyPolicy::WorkspaceAndPathDepsResident => {
                PackageResidencyPolicy::WorkspaceAndPathDepsResident
            }
            ProtoPackageResidencyPolicy::WorkspacePathAndDirectDepsResident => {
                PackageResidencyPolicy::WorkspacePathAndDirectDepsResident
            }
            ProtoPackageResidencyPolicy::AllOffloadable => PackageResidencyPolicy::AllOffloadable,
        };
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
        let indexing_preference = match config.indexing_preference {
            ProtoIndexingPerformancePreference::LowerPeakMemory => {
                IndexingPerformancePreference::LowerPeakMemory
            }
            ProtoIndexingPerformancePreference::FasterBuilds => {
                IndexingPerformancePreference::FasterBuilds
            }
        };
        let package_batch_size = PackageBatchSize::new(config.package_batch_size.get())
            .expect("protocol package batch size should remain positive");

        Self {
            package_residency_policy,
            cargo_metadata_config,
            workspace_lowering_config,
            indexing_preference,
            package_batch_size,
            discover_sysroot: matches!(config.sysroot_discovery, ProtoSysrootDiscovery::Auto),
        }
    }
}
