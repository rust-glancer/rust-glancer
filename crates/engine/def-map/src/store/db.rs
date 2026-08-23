//! Def-map package store and transaction entry points.

use crate::{DefMap, MacroExpansionLimitReport, PackageDefMaps};
use rg_ir_model::{CrateId, CrateRef};
use rg_item_tree::ItemTreeDb;
use rg_package_store::{PackageLoader, PackageStore, PackageSubset};
use rg_text::PackageNameInterners;

use crate::{DefMapBuildSession, DefMapReadTxn, MacroExpansionPerformancePreference, PackageSlot};
use rg_std::{MemorySize, Shrink};
use rg_workspace::{PackageOrigin, WorkspaceMetadata};

/// Frozen def maps for all parsed packages and semantic crates.
#[derive(Debug, Clone, PartialEq, Eq, Default, MemorySize)]
pub struct DefMapDb {
    packages: PackageStore<PackageDefMaps>,
}

impl DefMapDb {
    /// Starts replacing selected packages while retaining state across generated-source pauses.
    #[allow(clippy::too_many_arguments)]
    pub fn start_package_build(
        &self,
        baseline_read: &DefMapReadTxn<'_>,
        workspace: &WorkspaceMetadata,
        parse: &rg_parse::ParseDb,
        item_tree: &ItemTreeDb,
        packages: &[PackageSlot],
        interners: &mut PackageNameInterners,
        performance_preference: MacroExpansionPerformancePreference,
    ) -> anyhow::Result<DefMapBuildSession> {
        DefMapBuildSession::start(
            self,
            baseline_read,
            workspace,
            parse,
            item_tree,
            packages,
            interners,
            performance_preference,
        )
    }

    /// Builds a def-map database from an already shaped package store.
    ///
    /// Project construction starts with offloaded package slots and replaces every source-built
    /// package through a resumable build session. Artifact-backed loading can mix resident and
    /// offloaded slots after validating the workspace snapshot.
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

    /// Returns coarse DefMap totals for the resident package population.
    ///
    /// Offloaded packages are intentionally not loaded for reporting. Origin totals therefore use
    /// exactly the same resident crate maps as the aggregate count.
    pub fn stats(&self, workspace: &WorkspaceMetadata) -> DefMapStats {
        let mut stats = DefMapStats::default();

        for (_, entry) in self.packages.raw_entries_with_slots() {
            let Some(package) = entry.as_resident() else {
                continue;
            };
            stats.resident_package_count += 1;
            stats.macro_expansion_limit_crate_count += package.macro_expansion_limits().len();
        }

        for (crate_ref, def_map) in self.resident_crate_maps() {
            stats.crate_count += 1;
            stats.module_count += def_map.modules().len();
            stats.local_def_count += def_map.local_defs().len();
            stats.local_impl_count += def_map.local_impls().len();
            stats.import_count += def_map.imports().len();
            let unresolved = def_map
                .modules()
                .iter()
                .map(|module| module.unresolved_imports.len())
                .sum::<usize>();
            stats.unresolved_import_count += unresolved;

            let package = workspace
                .packages()
                .get(crate_ref.package.0)
                .expect("def-map package slot should match workspace metadata");
            match &package.origin {
                PackageOrigin::Workspace => {
                    stats.unresolved_imports_by_origin.workspace += unresolved;
                }
                PackageOrigin::Dependency => {
                    stats.unresolved_imports_by_origin.dependency += unresolved;
                }
                PackageOrigin::Sysroot(_) => {
                    stats.unresolved_imports_by_origin.sysroot += unresolved;
                }
            }
        }

        assert_eq!(
            stats.unresolved_import_count,
            stats.unresolved_imports_by_origin.total(),
            "origin-aware unresolved imports should equal the resident aggregate",
        );

        stats
    }

    /// Iterates bounded macro-limit diagnostics retained by resident packages.
    ///
    /// Status capture uses only the count in [`DefMapStats`]. Callers that intend to present the
    /// diagnostic details opt into walking these frozen reports explicitly.
    pub fn macro_expansion_limit_reports(
        &self,
    ) -> impl Iterator<Item = &MacroExpansionLimitReport> {
        self.packages
            .raw_entries_with_slots()
            .filter_map(|(_, entry)| entry.as_resident())
            .flat_map(|package| package.macro_expansion_limits().iter())
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

/// Coarse totals over the resident DefMap payloads used for status output.
///
/// The aggregate unresolved-import count and its origin split cover the same crate-map population.
/// Offloaded packages stay offloaded while these counters are captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, MemorySize)]
pub struct DefMapStats {
    pub resident_package_count: usize,
    pub crate_count: usize,
    pub module_count: usize,
    pub local_def_count: usize,
    pub local_impl_count: usize,
    pub import_count: usize,
    pub unresolved_import_count: usize,
    pub unresolved_imports_by_origin: UnresolvedImportStats,
    /// Resident crates with a retained macro-expansion-limit report.
    pub macro_expansion_limit_crate_count: usize,
}

/// Unresolved imports grouped by the role of the package containing the `use` item.
///
/// This is ownership, not a diagnosis of why resolution failed. For example,
/// `use core::missing;` written inside a dependency counts as dependency-owned even though the
/// path begins with a sysroot crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, MemorySize)]
pub struct UnresolvedImportStats {
    pub workspace: usize,
    pub dependency: usize,
    pub sysroot: usize,
}

impl UnresolvedImportStats {
    pub fn total(self) -> usize {
        self.workspace + self.dependency + self.sysroot
    }
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
            Vec::new(),
        )
    }
}
