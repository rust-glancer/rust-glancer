use std::path::Path;

use anyhow::Context as _;
use rg_ir_model::{CrateRef, PackageSlot};
use rg_package_store::PackageSubset;
use rg_parse::ParseDb;
use rg_std::{MemorySize, UniqueVec};
use rg_workspace::WorkspaceMetadata;

use super::subset;

/// Packages selected for one phase build, rebuild, or residency step.
///
/// The durable phase stores move by package slot, while item-tree construction still wants raw
/// package indices. Keeping both projections here prevents project lifecycle code from growing its
/// own subtly different package-set plumbing.
#[derive(Debug, Clone, PartialEq, Eq, Default, MemorySize)]
pub(super) struct PhasePackageSet {
    packages: Vec<PackageSlot>,
}

impl PhasePackageSet {
    pub(super) fn from_packages(packages: Vec<PackageSlot>) -> Self {
        Self { packages }
    }

    pub(super) fn from_slice(packages: &[PackageSlot]) -> Self {
        Self {
            packages: packages.to_vec(),
        }
    }

    pub(super) fn from_crates(crates: &[CrateRef]) -> Self {
        let mut packages = crates
            .iter()
            .map(|crate_ref| crate_ref.package)
            .collect::<UniqueVec<_>>()
            .into_vec();
        packages.sort_by_key(|package| package.0);
        Self { packages }
    }

    pub(super) fn from_path(parse: &ParseDb, path: &Path) -> anyhow::Result<Self> {
        let path = path
            .canonicalize()
            .with_context(|| format!("canonicalize {}", path.display()))?;
        // Shared source files can appear in several targets or packages. Keep each owner once.
        let mut packages = parse
            .file_refs_for_path(&path)
            .into_iter()
            .map(|file| PackageSlot(file.package))
            .collect::<Vec<_>>();
        packages.sort_unstable();
        packages.dedup();
        Ok(Self { packages })
    }

    pub(super) fn as_slice(&self) -> &[PackageSlot] {
        &self.packages
    }

    pub(super) fn into_vec(self) -> Vec<PackageSlot> {
        self.packages
    }

    pub(super) fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }

    pub(super) fn contains(&self, package: PackageSlot) -> bool {
        self.packages.contains(&package)
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = PackageSlot> + '_ {
        self.packages.iter().copied()
    }

    pub(super) fn filter(&self, mut predicate: impl FnMut(PackageSlot) -> bool) -> Self {
        Self {
            packages: self
                .packages
                .iter()
                .copied()
                .filter(|&package| predicate(package))
                .collect(),
        }
    }

    pub(super) fn package_indices(&self) -> Vec<usize> {
        self.packages.iter().map(|package| package.0).collect()
    }

    pub(super) fn visible_dependency_subset(&self, workspace: &WorkspaceMetadata) -> PackageSubset {
        // Source-built packages can resolve names through visible dependencies, including packages
        // that were startup-cache hits. The subset tells lazy package stores which offloaded
        // packages are valid reads during this coherent build.
        subset::rebuild_packages_with_visible_dependencies(workspace, &self.packages)
    }
}

#[cfg(test)]
mod tests {
    use rg_ir_model::{CrateId, CrateRef, PackageSlot};

    use super::PhasePackageSet;

    #[test]
    fn crate_sets_are_sorted_and_deduplicated_by_package() {
        let crates = [(2, 0), (0, 0), (2, 1), (1, 0)].map(|(package, crate_id)| CrateRef {
            package: PackageSlot(package),
            crate_id: CrateId(crate_id),
        });
        let set = PhasePackageSet::from_crates(&crates);
        assert_eq!(
            set.as_slice(),
            &[PackageSlot(0), PackageSlot(1), PackageSlot(2)]
        );
    }

    #[test]
    fn filtering_preserves_phase_package_order() {
        let set = PhasePackageSet::from_packages(vec![
            PackageSlot(3),
            PackageSlot(1),
            PackageSlot(4),
            PackageSlot(1),
        ]);

        let filtered = set.filter(|package| package.0 % 2 == 1);

        assert_eq!(
            filtered.as_slice(),
            &[PackageSlot(3), PackageSlot(1), PackageSlot(1)]
        );
    }
}
