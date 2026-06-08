use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use rg_cfg_eval::CfgOptions;
use rg_memsize::MemorySize;

use crate::{SysrootCrate, SysrootSources};

use super::{
    dependency::PackageDependency,
    edition::RustEdition,
    package::{Package, PackageId, PackageOrigin, PackageSource},
    target::{Target, TargetKind},
};

/// Normalized workspace metadata used by the analysis pipeline.
///
/// It keeps only the fields and semantics the later phases care about. Filesystem roots are
/// canonicalized at construction so save handling can compare paths directly without each phase
/// defending against the original path spelling. Missing non-workspace targets are omitted because
/// they cannot be parsed or reached through local save handling.
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
    pub(crate) fn from_parts(
        workspace_root: PathBuf,
        target_cfg_options: CfgOptions,
        packages: Vec<Package>,
    ) -> Self {
        let package_by_id = Self::package_index(&packages);
        Self {
            workspace_root,
            target_cfg_options,
            packages,
            package_by_id,
        }
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
        self.package_by_id = Self::package_index(&self.packages);
    }

    fn package_index(packages: &[Package]) -> HashMap<PackageId, usize> {
        packages
            .iter()
            .enumerate()
            .map(|(idx, package)| (package.id.clone(), idx))
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
