//! Def-map package store and transaction entry points.

use crate::{DefMap, PackageDefMaps};
use rg_ir_model::{CrateId, CrateRef};
use rg_item_tree::ItemTreeDb;
use rg_package_store::{PackageLoader, PackageStore, PackageSubset};
use rg_text::PackageNameInterners;
use rg_workspace::WorkspaceMetadata;

use crate::{
    DefMapReadTxn, PackageSlot,
    build::{DefMapDbBuilder, DefMapDbPackageRebuilder},
};
use rg_std::{MemorySize, Shrink};

/// Frozen def maps for all parsed packages and semantic crates.
#[derive(Debug, Clone, PartialEq, Eq, Default, MemorySize)]
pub struct DefMapDb {
    packages: PackageStore<PackageDefMaps>,
}

impl DefMapDb {
    /// Starts building crate-local def maps from parsed metadata and lowered item trees.
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

    pub(crate) fn from_packages(packages: Vec<PackageDefMaps>) -> Self {
        Self::from_package_store(PackageStore::from_vec(packages))
    }

    /// Builds a def-map database from an already shaped package store.
    ///
    /// Fresh builds use `from_packages`, while artifact-backed loading can construct resident and
    /// offloaded package slots directly after validating the workspace snapshot.
    pub fn from_package_store(packages: PackageStore<PackageDefMaps>) -> Self {
        Self { packages }
    }

    pub(crate) fn mutator(&mut self) -> DefMapDbMutator<'_> {
        DefMapDbMutator { db: self }
    }

    /// Returns the number of package slots tracked by this snapshot.
    pub fn package_count(&self) -> usize {
        self.packages.len()
    }

    /// Iterates over every resident crate def map together with its project-wide crate reference.
    fn resident_crate_maps(&self) -> impl Iterator<Item = (CrateRef, &DefMap)> {
        self.packages
            .raw_entries_with_slots()
            .filter_map(|(package_slot, entry)| {
                entry.as_resident().map(|package| (package_slot, package))
            })
            .flat_map(move |(package_slot, package)| {
                package
                    .crates()
                    .iter()
                    .enumerate()
                    .map(move |(crate_idx, data)| {
                        let crate_ref = CrateRef {
                            package: package_slot,
                            crate_id: CrateId(crate_idx),
                        };
                        (crate_ref, data.def_map())
                    })
            })
    }

    /// Returns coarse DefMap totals for the current project report.
    pub fn stats(&self) -> DefMapStats {
        let mut stats = DefMapStats::default();

        for (_, def_map) in self.resident_crate_maps() {
            stats.crate_count += 1;
            stats.module_count += def_map.modules().len();
            stats.local_def_count += def_map.local_defs().len();
            stats.local_impl_count += def_map.local_impls().len();
            stats.import_count += def_map.imports().len();
            stats.unresolved_import_count += def_map
                .modules()
                .iter()
                .map(|module| module.unresolved_imports.len())
                .sum::<usize>();
        }

        stats
    }

    /// Returns one resident package def-map set by package slot.
    pub fn resident_package(&self, package_slot: PackageSlot) -> Option<&PackageDefMaps> {
        self.packages
            .raw_entry(package_slot)
            .and_then(|entry| entry.as_resident())
    }

    /// Replaces one package payload while preserving the surrounding package-store shape.
    ///
    /// Exact on-demand Body IR rebuilds use this to temporarily restore an artifact-backed
    /// package. Rewriting that artifact requires every phase payload to be resident together.
    pub fn replace_package(
        &mut self,
        package_slot: PackageSlot,
        package: PackageDefMaps,
    ) -> Option<()> {
        self.packages.replace(package_slot, package)
    }

    pub fn read_txn<'db>(
        &'db self,
        loader: PackageLoader<'db, PackageDefMaps>,
    ) -> DefMapReadTxn<'db> {
        DefMapReadTxn::from_package_store(self.packages.read_txn(loader))
    }

    pub fn read_txn_for_subset<'db>(
        &'db self,
        loader: PackageLoader<'db, PackageDefMaps>,
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
        package: PackageDefMaps,
    ) -> Option<()> {
        self.db.replace_package(package_slot, package)
    }

    pub(crate) fn compact_storage(&mut self) {
        Shrink::shrink_to_fit(&mut self.db.packages);
    }

    pub(crate) fn compact_packages(&mut self, packages: &[PackageSlot]) {
        // Build compact package copies before replacing the source packages. This keeps the final
        // allocations grouped together instead of interleaving each shrink allocation with frees
        // from the same package, which can leave sparse allocator slabs after large rebuilds.
        let compacted_packages = packages
            .iter()
            .filter_map(|package| {
                let mut compacted = self.db.resident_package(*package)?.clone();
                Shrink::shrink_to_fit(&mut compacted);
                Some((*package, compacted))
            })
            .collect::<Vec<_>>();

        for (package, compacted) in compacted_packages {
            self.replace_package(package, compacted);
        }
    }
}

/// Coarse totals for reporting that the DefMap phase produced useful data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, MemorySize)]
pub struct DefMapStats {
    pub crate_count: usize,
    pub module_count: usize,
    pub local_def_count: usize,
    pub local_impl_count: usize,
    pub import_count: usize,
    pub unresolved_import_count: usize,
}

#[cfg(test)]
mod tests {
    use crate::{CrateData, DefMapBuilder};
    use rg_ir_model::CrateRef;
    use rg_parse::CargoTargetId;

    use super::*;

    #[test]
    fn crate_maps_preserve_package_slots_when_middle_package_is_offloaded() {
        let mut db = DefMapDb {
            packages: PackageStore::from_vec(vec![
                package_with_one_crate("workspace"),
                package_with_one_crate("offloaded"),
                package_with_one_crate("dependency"),
            ]),
        };

        db.offload_package(PackageSlot(1))
            .expect("middle package should exist");

        let crate_packages = db
            .resident_crate_maps()
            .map(|(crate_ref, _)| crate_ref.package)
            .collect::<Vec<_>>();

        assert_eq!(crate_packages, vec![PackageSlot(0), PackageSlot(2)]);
    }

    fn package_with_one_crate(name: &str) -> PackageDefMaps {
        let crate_ref = CrateRef {
            package: PackageSlot(0),
            crate_id: CrateId(0),
        };
        PackageDefMaps::new(
            name.to_string(),
            rg_text::RustEdition::Edition2024,
            vec![CrateData::new(
                CargoTargetId(0),
                rg_workspace::TargetKind::Lib,
                format!("{name}_lib"),
                None,
                Default::default(),
                None,
                DefMapBuilder::new(crate_ref).build(),
            )],
        )
    }
}
