use rg_body_ir::BodyIrFile;
use rg_def_map::PackageSlot;
use rg_package_store::PackageSubset;
use rg_std::MemorySize;
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
    pub(super) fn all(package_count: usize) -> Self {
        Self {
            packages: (0..package_count).map(PackageSlot).collect(),
        }
    }

    pub(super) fn from_packages(packages: Vec<PackageSlot>) -> Self {
        Self { packages }
    }

    pub(super) fn from_slice(packages: &[PackageSlot]) -> Self {
        Self {
            packages: packages.to_vec(),
        }
    }

    pub(super) fn from_body_files(files: &[BodyIrFile]) -> Self {
        let mut packages = files.iter().map(|file| file.package).collect::<Vec<_>>();
        packages.sort_by_key(|package| package.0);
        packages.dedup();
        Self { packages }
    }

    pub(super) fn as_slice(&self) -> &[PackageSlot] {
        &self.packages
    }

    pub(super) fn is_empty(&self) -> bool {
        self.packages.is_empty()
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
    use rg_parse::FileId;

    use super::*;

    #[test]
    fn package_sets_project_to_package_indices() {
        let cases = [
            ("all packages", PhasePackageSet::all(3), vec![0, 1, 2]),
            (
                "explicit package order",
                PhasePackageSet::from_packages(vec![
                    PackageSlot(3),
                    PackageSlot(1),
                    PackageSlot(4),
                ]),
                vec![3, 1, 4],
            ),
        ];

        for (case, set, expected_indices) in cases {
            assert_eq!(set.package_indices(), expected_indices, "{case}");
        }
    }

    #[test]
    fn body_file_sets_are_sorted_and_deduplicated() {
        let files = [
            BodyIrFile::new(PackageSlot(2), FileId(0)),
            BodyIrFile::new(PackageSlot(0), FileId(1)),
            BodyIrFile::new(PackageSlot(2), FileId(2)),
            BodyIrFile::new(PackageSlot(1), FileId(3)),
        ];

        let set = PhasePackageSet::from_body_files(&files);

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
