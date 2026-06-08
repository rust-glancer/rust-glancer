use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use rg_cfg_eval::CfgOptions;
use serde::{Deserialize, Serialize};

use crate::{
    RustcTarget, WorkspaceMetadataError, WorkspaceMetadataResult, path::canonicalize_path,
};
use rg_std::MemorySize;

/// Options used when asking Cargo for the workspace graph.
///
/// Cargo metadata includes dependencies for every platform unless callers pass
/// `--filter-platform`. Analysis wants one concrete graph, so the default resolves the current
/// rustc host triple and lets Cargo prune target-specific dependencies before lowering starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, MemorySize)]
pub struct CargoMetadataConfig {
    target: CargoMetadataTarget,
}

impl CargoMetadataConfig {
    /// Uses an explicit target triple instead of auto-detecting the rustc host target.
    pub fn target_triple(mut self, target_triple: impl Into<String>) -> Self {
        self.target = RustcTarget::new(target_triple)
            .map(CargoMetadataTarget::Triple)
            .unwrap_or(CargoMetadataTarget::Auto);
        self
    }

    /// Returns the configured target selection before auto-detection is resolved.
    pub fn target(&self) -> &CargoMetadataTarget {
        &self.target
    }

    /// Runs `cargo metadata` and returns the target cfg environment used for normalization.
    pub fn load_metadata_with_target_cfg(
        &self,
        manifest_path: impl AsRef<Path>,
    ) -> WorkspaceMetadataResult<LoadedCargoMetadata> {
        let target = self.resolved_target()?;
        let target_cfg = target.cfg_options()?;
        let metadata = self
            .metadata_command_for_target(manifest_path.as_ref(), &target)
            .exec()
            .map_err(WorkspaceMetadataError::CargoMetadata)?;

        Ok(LoadedCargoMetadata {
            metadata,
            target_cfg,
        })
    }

    /// Runs `cargo metadata --no-deps` and returns canonical workspace member manifests.
    ///
    /// This gives update code a cheap Cargo-backed discovery probe for new workspace members
    /// without constructing a partial analysis graph or resolving third-party dependencies.
    pub fn load_workspace_member_manifest_paths(
        &self,
        manifest_path: impl AsRef<Path>,
    ) -> WorkspaceMetadataResult<Vec<PathBuf>> {
        let target = self.resolved_target()?;
        let metadata = self
            .metadata_command_for_target(manifest_path.as_ref(), &target)
            .no_deps()
            .exec()
            .map_err(WorkspaceMetadataError::CargoMetadata)?;
        let workspace_members = metadata
            .workspace_members
            .iter()
            .map(ToString::to_string)
            .collect::<HashSet<_>>();

        metadata
            .packages
            .into_iter()
            .filter(|package| workspace_members.contains(&package.id.to_string()))
            .map(|package| {
                canonicalize_path(package.manifest_path.as_std_path())
                    .map_err(WorkspaceMetadataError::Path)
            })
            .collect()
    }

    fn metadata_command_for_target(
        &self,
        manifest_path: &Path,
        target: &RustcTarget,
    ) -> cargo_metadata::MetadataCommand {
        let mut command = cargo_metadata::MetadataCommand::new();
        command.manifest_path(manifest_path.to_path_buf());
        command.other_options(vec![
            "--filter-platform".to_string(),
            target.as_str().to_string(),
        ]);
        command
    }

    fn resolved_target(&self) -> WorkspaceMetadataResult<RustcTarget> {
        match &self.target {
            CargoMetadataTarget::Auto => RustcTarget::detect_host(),
            CargoMetadataTarget::Triple(target) => Ok(target.clone()),
        }
    }
}

impl Default for CargoMetadataConfig {
    fn default() -> Self {
        Self {
            target: CargoMetadataTarget::Auto,
        }
    }
}

/// Target platform selection for Cargo metadata filtering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, MemorySize)]
pub enum CargoMetadataTarget {
    /// Detect the host triple from `rustc -vV`.
    Auto,
    /// Pass this target triple to `cargo metadata --filter-platform`.
    Triple(RustcTarget),
}

/// Raw Cargo metadata plus the resolved target cfg facts it should be lowered with.
#[derive(Debug)]
pub struct LoadedCargoMetadata {
    pub metadata: cargo_metadata::Metadata,
    pub target_cfg: CfgOptions,
}
