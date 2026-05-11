//! Def-map package store and transaction entry points.

use rg_item_tree::ItemTreeDb;
use rg_memsize::{MemoryRecorder, MemorySize};
use rg_package_store::{PackageLoader, PackageStore, PackageSubset};
use rg_parse::{self, TargetId};
use rg_text::PackageNameInterners;
use rg_workspace::WorkspaceMetadata;

use crate::{
    DefMap, DefMapReadTxn, Package, PackageSlot,
    build::{DefMapDbBuilder, DefMapDbPackageRebuilder},
    ids::ResidentTargetRef,
};

/// Frozen def maps for all parsed packages and targets.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DefMapDb {
    packages: PackageStore<Package>,
}

impl DefMapDb {
    /// Starts building target-local def maps from parsed metadata and lowered item trees.
    pub fn builder<'a>(
        workspace: &'a WorkspaceMetadata,
        parse: &'a rg_parse::ParseDb,
        item_tree: &'a ItemTreeDb,
    ) -> DefMapDbBuilder<'a, 'static> {
        DefMapDbBuilder::new(workspace, parse, item_tree)
    }

    /// Starts rebuilding selected packages against a logical old def-map view.
    pub fn package_rebuilder<'a, 'db>(
        &'a self,
        old_read: &'a DefMapReadTxn<'db>,
        workspace: &'a WorkspaceMetadata,
        parse: &'a rg_parse::ParseDb,
        item_tree: &'a ItemTreeDb,
        packages: &'a [PackageSlot],
        interners: &'a mut PackageNameInterners,
    ) -> DefMapDbPackageRebuilder<'a, 'db> {
        DefMapDbPackageRebuilder::new(
            self, old_read, workspace, parse, item_tree, packages, interners,
        )
    }

    pub(crate) fn record_packages_memory_children(&self, recorder: &mut MemoryRecorder) {
        self.packages.record_memory_children(recorder);
    }

    pub(crate) fn from_packages(packages: Vec<Package>) -> Self {
        Self::from_package_store(PackageStore::from_vec(packages))
    }

    /// Builds a def-map database from an already shaped package store.
    ///
    /// Fresh builds use `from_packages`, while artifact-backed loading can construct resident and
    /// offloaded package slots directly after validating the workspace snapshot.
    pub fn from_package_store(packages: PackageStore<Package>) -> Self {
        Self { packages }
    }

    pub(crate) fn mutator(&mut self) -> DefMapDbMutator<'_> {
        DefMapDbMutator { db: self }
    }

    /// Returns the number of package slots tracked by this snapshot.
    pub fn package_count(&self) -> usize {
        self.packages.len()
    }

    /// Iterates over every resident target def map together with a resident-only target reference.
    fn resident_target_maps(&self) -> impl Iterator<Item = (ResidentTargetRef, &DefMap)> {
        self.packages
            .raw_entries_with_slots()
            .filter_map(|(package_slot, entry)| {
                entry.as_resident().map(|package| (package_slot, package))
            })
            .flat_map(move |(package_slot, package)| {
                package
                    .targets()
                    .iter()
                    .enumerate()
                    .map(move |(target_idx, def_map)| {
                        let target_ref = ResidentTargetRef {
                            package: package_slot,
                            target: TargetId(target_idx),
                        };
                        (target_ref, def_map)
                    })
            })
    }

    /// Returns coarse DefMap totals for the current project report.
    pub fn stats(&self) -> DefMapStats {
        let mut stats = DefMapStats::default();

        for (_, target) in self.resident_target_maps() {
            stats.target_count += 1;
            stats.module_count += target.modules().len();
            stats.local_def_count += target.local_defs().len();
            stats.local_impl_count += target.local_impls().len();
            stats.import_count += target.imports().len();
            stats.unresolved_import_count += target
                .modules()
                .iter()
                .map(|module| module.unresolved_imports.len())
                .sum::<usize>();
        }

        stats
    }

    /// Returns one resident package def-map set by package slot.
    pub fn resident_package(&self, package_slot: PackageSlot) -> Option<&Package> {
        self.packages
            .raw_entry(package_slot)
            .and_then(|entry| entry.as_resident())
    }

    pub fn read_txn<'db>(&'db self, loader: PackageLoader<'db, Package>) -> DefMapReadTxn<'db> {
        DefMapReadTxn::from_package_store(self.packages.read_txn(loader))
    }

    pub fn read_txn_for_subset<'db>(
        &'db self,
        loader: PackageLoader<'db, Package>,
        subset: &PackageSubset,
    ) -> DefMapReadTxn<'db> {
        DefMapReadTxn::from_package_store(self.packages.read_txn_for_subset(loader, subset))
    }

    pub fn offload_package(&mut self, package_slot: PackageSlot) -> Option<()> {
        self.packages.offload(package_slot)
    }
}

pub(crate) struct DefMapDbMutator<'db> {
    db: &'db mut DefMapDb,
}

impl DefMapDbMutator<'_> {
    pub(crate) fn replace_package(
        &mut self,
        package_slot: PackageSlot,
        package: Package,
    ) -> Option<()> {
        self.db.packages.replace(package_slot, package)
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.db.packages.shrink_to_fit();
        for entry in self.db.packages.raw_entries_mut() {
            if let Some(package) = entry.as_resident_unique_mut() {
                package.shrink_to_fit();
            }
        }
    }

    pub(crate) fn shrink_packages(&mut self, packages: &[PackageSlot]) {
        for package in packages {
            if let Some(package) = self.db.packages.get_unique_mut(*package) {
                package.shrink_to_fit();
            }
        }
    }
}

/// Coarse totals for reporting that the DefMap phase produced useful data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DefMapStats {
    pub target_count: usize,
    pub module_count: usize,
    pub local_def_count: usize,
    pub local_impl_count: usize,
    pub import_count: usize,
    pub unresolved_import_count: usize,
}

#[cfg(test)]
mod tests {
    use rg_arena::Arena;

    use super::*;

    #[test]
    fn target_maps_preserve_package_slots_when_middle_package_is_offloaded() {
        let mut db = DefMapDb {
            packages: PackageStore::from_vec(vec![
                package_with_one_target("workspace"),
                package_with_one_target("offloaded"),
                package_with_one_target("dependency"),
            ]),
        };

        db.offload_package(PackageSlot(1))
            .expect("middle package should exist");

        let target_packages = db
            .resident_target_maps()
            .map(|(target, _)| target.package)
            .collect::<Vec<_>>();

        assert_eq!(target_packages, vec![PackageSlot(0), PackageSlot(2)]);
    }

    fn package_with_one_target(name: &str) -> Package {
        Package {
            name: name.to_string(),
            target_names: Arena::from_vec(vec![format!("{name}_lib")]),
            targets: Arena::from_vec(vec![DefMap::default()]),
        }
    }
}
