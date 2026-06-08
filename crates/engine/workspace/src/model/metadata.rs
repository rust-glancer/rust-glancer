use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use rg_cfg_eval::CfgOptions;
use rg_memsize::MemorySize;

use crate::{
    CargoMetadataConfig, SysrootCrate, SysrootSources, WorkspaceMetadataError,
    WorkspaceMetadataResult, canonicalize_path, cfg_options_from_rustc_target,
};

use super::{
    dependency::PackageDependency,
    edition::RustEdition,
    package::{Package, PackageId, PackageOrigin, PackageSource},
    target::{Target, TargetKind},
};

/// Normalized workspace metadata used by the analysis pipeline.
///
/// This is our internal view of `cargo metadata`: it keeps only the fields and semantics the
/// later phases care about and avoids leaking Cargo's transport types throughout the codebase.
/// Filesystem roots are canonicalized at construction so save handling can compare paths directly
/// without each phase defending against Cargo's original path spelling. Missing non-workspace
/// targets are omitted because they cannot be parsed or reached through local save handling.
#[derive(Debug, Clone, PartialEq, Eq, MemorySize)]
pub struct WorkspaceMetadata {
    workspace_root: PathBuf,
    // Target/platform cfg facts are kept separate from package cfgs, which additionally include
    // Cargo features active for that package.
    target_cfg_options: CfgOptions,
    packages: Vec<Package>,
    package_by_id: HashMap<PackageId, usize>,
}

impl WorkspaceMetadata {
    /// Loads Cargo metadata from a manifest path and lowers it into the analysis metadata model.
    pub fn from_manifest_path(manifest_path: impl AsRef<Path>) -> WorkspaceMetadataResult<Self> {
        Self::from_manifest_path_with_config(manifest_path, &CargoMetadataConfig::default())
    }

    /// Loads Cargo metadata with explicit metadata options and lowers it into the internal model.
    pub fn from_manifest_path_with_config(
        manifest_path: impl AsRef<Path>,
        config: &CargoMetadataConfig,
    ) -> WorkspaceMetadataResult<Self> {
        let target_triple = config.resolved_target_triple()?;
        let target_cfg = cfg_options_from_rustc_target(&target_triple)?;
        let metadata = config
            .metadata_command_for_target(manifest_path.as_ref(), &target_triple)?
            .exec()
            .map_err(WorkspaceMetadataError::CargoMetadata)?;

        Self::from_cargo_with_target_cfg(metadata, target_cfg)
    }

    /// Lowers raw `cargo metadata` output into the project's normalized metadata model.
    pub fn from_cargo(metadata: cargo_metadata::Metadata) -> WorkspaceMetadataResult<Self> {
        Self::from_cargo_with_target_cfg(metadata, CfgOptions::current_host())
    }

    /// Lowers raw Cargo metadata with an already-resolved target cfg environment.
    pub fn from_cargo_with_target_cfg(
        metadata: cargo_metadata::Metadata,
        target_cfg: CfgOptions,
    ) -> WorkspaceMetadataResult<Self> {
        let workspace_root = canonicalize_path(metadata.workspace_root.as_std_path())
            .map_err(WorkspaceMetadataError::Path)?;
        let workspace_members = metadata
            .workspace_members
            .iter()
            .map(PackageId::from_cargo)
            .collect::<HashSet<_>>();
        let dependencies_by_package = metadata
            .resolve
            .as_ref()
            .map(Self::lower_dependencies)
            .unwrap_or_default();
        let features_by_package = metadata
            .resolve
            .as_ref()
            .map(Self::lower_active_features)
            .unwrap_or_default();

        let packages = metadata
            .packages
            .into_iter()
            .map(|package| {
                let package_id = PackageId::from_cargo(&package.id);
                let mut cfg_options = target_cfg.clone();
                for feature in features_by_package.get(&package_id).into_iter().flatten() {
                    cfg_options.insert_key_value("feature", feature);
                }
                let is_workspace_member = workspace_members.contains(&package_id);
                let raw_manifest_path = package.manifest_path.as_std_path();
                let manifest_path =
                    canonicalize_path(raw_manifest_path).map_err(WorkspaceMetadataError::Path)?;
                let raw_package_root = raw_manifest_path
                    .parent()
                    .expect("Cargo package manifest path should have a parent directory");
                let package_root = manifest_path
                    .parent()
                    .expect("canonical package manifest path should have a parent directory");
                let source = PackageSource::from_cargo_source(
                    &package_id,
                    is_workspace_member,
                    package.source.as_ref(),
                )?;
                let targets = package
                    .targets
                    .iter()
                    .map(|target| {
                        Target::from_cargo(
                            target,
                            raw_package_root,
                            package_root,
                            is_workspace_member,
                        )
                    })
                    .collect::<WorkspaceMetadataResult<Vec<_>>>()?
                    .into_iter()
                    .flatten()
                    .collect();

                Ok(Package {
                    id: package_id.clone(),
                    name: package.name.to_string(),
                    edition: RustEdition::from_cargo(package.edition),
                    origin: if is_workspace_member {
                        PackageOrigin::Workspace
                    } else {
                        PackageOrigin::Dependency
                    },
                    source,
                    is_workspace_member,
                    manifest_path,
                    cfg_options,
                    targets,
                    dependencies: dependencies_by_package
                        .get(&package_id)
                        .cloned()
                        .unwrap_or_default(),
                })
            })
            .collect::<WorkspaceMetadataResult<Vec<_>>>()?;

        let package_by_id = packages
            .iter()
            .enumerate()
            .map(|(idx, package)| (package.id.clone(), idx))
            .collect();

        Ok(Self {
            workspace_root,
            target_cfg_options: target_cfg,
            packages,
            package_by_id,
        })
    }

    /// Returns this workspace with sysroot crates modeled as ordinary packages.
    pub fn with_sysroot_sources(mut self, sources: Option<SysrootSources>) -> Self {
        if let Some(sources) = sources {
            self.add_sysroot_sources(sources);
        }
        self
    }

    /// Returns sysroot source roots that were previously injected into this metadata graph.
    pub fn sysroot_sources(&self) -> Option<SysrootSources> {
        self.packages
            .iter()
            .find(|package| package.origin.is_sysroot())
            .and_then(|package| package.manifest_path.parent())
            .and_then(|crate_root| crate_root.parent())
            .and_then(SysrootSources::from_library_root)
    }

    /// Adds `core`, `alloc`, and `std` from rust-src and injects them into normal packages.
    pub fn add_sysroot_sources(&mut self, sources: SysrootSources) {
        if self
            .packages
            .iter()
            .any(|package| package.origin.is_sysroot())
        {
            return;
        }

        let target_cfg = self.target_cfg_options.clone();
        let mut sysroot_packages = SysrootCrate::ALL
            .iter()
            .copied()
            .map(|krate| Self::sysroot_package(&sources, krate, target_cfg.clone()))
            .collect::<Vec<_>>();

        for package in &mut self.packages {
            if package.origin.is_sysroot() {
                continue;
            }

            for krate in SysrootCrate::ALL {
                if package
                    .dependencies
                    .iter()
                    .any(|dependency| dependency.name() == krate.name())
                {
                    continue;
                }
                package
                    .dependencies
                    .push(PackageDependency::for_all_targets(
                        PackageId::sysroot(krate),
                        krate.name(),
                    ));
            }
        }

        self.packages.append(&mut sysroot_packages);
        self.rebuild_package_index();
    }

    fn sysroot_package(
        sources: &SysrootSources,
        krate: SysrootCrate,
        cfg_options: CfgOptions,
    ) -> Package {
        let dependencies = match krate {
            SysrootCrate::Core => Vec::new(),
            SysrootCrate::Alloc => vec![PackageDependency::normal(
                PackageId::sysroot(SysrootCrate::Core),
                "core",
            )],
            SysrootCrate::Std => vec![
                PackageDependency::normal(PackageId::sysroot(SysrootCrate::Core), "core"),
                PackageDependency::normal(PackageId::sysroot(SysrootCrate::Alloc), "alloc"),
            ],
        };

        Package {
            id: PackageId::sysroot(krate),
            name: krate.name().to_string(),
            edition: RustEdition::Edition2024,
            origin: PackageOrigin::Sysroot(krate),
            source: PackageSource::Sysroot,
            is_workspace_member: false,
            manifest_path: sources.library_root().join(krate.name()).join("Cargo.toml"),
            cfg_options,
            targets: vec![Target {
                name: krate.name().to_string(),
                kind: TargetKind::Lib,
                src_path: sources.crate_root(krate),
            }],
            dependencies,
        }
    }

    fn rebuild_package_index(&mut self) {
        self.package_by_id = self
            .packages
            .iter()
            .enumerate()
            .map(|(idx, package)| (package.id.clone(), idx))
            .collect();
    }

    fn lower_dependencies(
        resolve: &cargo_metadata::Resolve,
    ) -> HashMap<PackageId, Vec<PackageDependency>> {
        resolve
            .nodes
            .iter()
            .map(|node| {
                (
                    PackageId::from_cargo(&node.id),
                    node.deps
                        .iter()
                        .map(PackageDependency::from_cargo)
                        .collect::<Vec<_>>(),
                )
            })
            .collect()
    }

    fn lower_active_features(resolve: &cargo_metadata::Resolve) -> HashMap<PackageId, Vec<String>> {
        resolve
            .nodes
            .iter()
            .map(|node| {
                let mut features = node
                    .features
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                features.sort();
                features.dedup();
                (PackageId::from_cargo(&node.id), features)
            })
            .collect()
    }

    /// Returns the workspace root directory.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Returns all known packages.
    pub fn packages(&self) -> &[Package] {
        &self.packages
    }

    /// Returns one package by normalized package id.
    pub fn package(&self, package_id: &PackageId) -> Option<&Package> {
        let slot = self.package_by_id.get(package_id).copied()?;
        self.packages.get(slot)
    }

    /// Returns package slots whose manifest directory contains `path`.
    ///
    /// This is intentionally a filesystem-root query, not a parsed-file ownership query. The
    /// analysis host uses it when a saved file was not part of the parsed graph yet, for example
    /// after `mod api;` was saved before `api.rs` existed. Rebuilding the containing package lets
    /// normal module discovery decide whether the new path is actually reachable.
    pub fn package_slots_containing_path(&self, path: &Path) -> Vec<usize> {
        self.packages
            .iter()
            .enumerate()
            .filter_map(|(slot, package)| package.contains_path(path).then_some(slot))
            .collect()
    }

    /// Iterates over packages that belong to the analyzed workspace.
    pub fn workspace_packages(&self) -> impl Iterator<Item = &Package> + '_ {
        self.packages
            .iter()
            .filter(|package| package.is_workspace_member)
    }

    /// Returns package slots that should be refreshed after one or more packages change.
    ///
    /// Source changes can alter the public surface of the changed package, so every reverse
    /// dependent must be rebuilt against the new graph. The closure is intentionally package-wide:
    /// it is coarse enough to stay predictable while avoiding whole-workspace rebuilds on normal
    /// source edits.
    pub fn reverse_dependency_closure(&self, roots: &[PackageId]) -> Vec<usize> {
        let mut affected_ids = roots.iter().cloned().collect::<HashSet<_>>();

        loop {
            let previous_len = affected_ids.len();

            for package in &self.packages {
                if affected_ids.contains(&package.id) {
                    continue;
                }

                if package
                    .dependencies
                    .iter()
                    .any(|dependency| affected_ids.contains(dependency.package_id()))
                {
                    affected_ids.insert(package.id.clone());
                }
            }

            if affected_ids.len() == previous_len {
                break;
            }
        }

        self.packages
            .iter()
            .enumerate()
            .filter_map(|(package_slot, package)| {
                affected_ids.contains(&package.id).then_some(package_slot)
            })
            .collect()
    }
}
