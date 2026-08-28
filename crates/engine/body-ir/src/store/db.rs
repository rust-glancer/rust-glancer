//! Body IR package store and transaction entry points.

use rg_def_map::{DefMapLoader, PackageSlot};
use rg_ir_model::CrateRef;
use rg_package_store::{PackageStore, PackageSubset};
use rg_semantic_ir::SemanticIrLoader;
use rg_std::MemorySize;
use rg_text::PackageNameInterners;

use super::{
    BodyIrLoader, BodyIrReadTxn, CrateBodiesCoverage, CrateBodiesStatus, PackageBodies,
    PackageBodiesCoverage,
};
use crate::build::BodyIrDbBuilder;

/// Coarse totals for reporting that the Body IR phase produced useful data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, MemorySize)]
pub struct BodyIrStats {
    pub crate_count: usize,
    pub built_crate_count: usize,
    pub skipped_crate_count: usize,
    pub complete_crate_count: usize,
    pub partial_crate_count: usize,
    pub missing_crate_count: usize,
    pub skipped_by_policy_crate_count: usize,
    pub body_count: usize,
    pub scope_count: usize,
    pub binding_count: usize,
    pub statement_count: usize,
    pub expression_count: usize,
}

/// Body-level IR for all analyzed packages and semantic crates.
///
/// Resident entries own their body arenas and coverage together. Offloaded entries replace that
/// payload with only [`PackageBodiesCoverage`], which is enough to decide whether a query needs
/// on-demand materialization before its read transaction opens the package artifact.
#[derive(Debug, Clone, PartialEq, Eq, Default, MemorySize)]
pub struct BodyIrDb {
    packages: PackageStore<PackageBodies, PackageBodiesCoverage>,
}

impl BodyIrDb {
    /// Starts replacing selected packages on top of this snapshot.
    ///
    /// `packages` selects what to rebuild. `copy_compact_packages` selects which rebuilt payloads
    /// should be cloned and shrunk for long-term residency; passing `packages` for both arguments
    /// requests compact retained output for every rebuilt package. The returned builder still
    /// requires one explicit materialization selection before it can build.
    #[allow(clippy::too_many_arguments)]
    pub fn builder<'db, 'names>(
        &'db self,
        parse: &'db rg_parse::ParseDb,
        def_map: &'db rg_def_map::DefMapDb,
        semantic_ir: &'db rg_semantic_ir::SemanticIrDb,
        packages: &'db [PackageSlot],
        copy_compact_packages: &[PackageSlot],
        interners: &'names mut PackageNameInterners,
        def_map_loader: DefMapLoader<'db>,
        semantic_ir_loader: SemanticIrLoader<'db>,
        subset: &'db PackageSubset,
    ) -> BodyIrDbBuilder<'db, 'names> {
        BodyIrDbBuilder::new(
            self,
            parse,
            def_map,
            semantic_ir,
            packages,
            copy_compact_packages,
            interners,
            def_map_loader,
            semantic_ir_loader,
            subset,
        )
    }

    /// Builds a Body IR database from an already shaped package store.
    ///
    /// Startup cache loading supplies validated coverage inside each offloaded entry, while source
    /// builds replace their provisional entries with ordinary resident payloads.
    pub fn from_package_store(
        packages: PackageStore<PackageBodies, PackageBodiesCoverage>,
    ) -> Self {
        Self { packages }
    }

    pub(crate) fn mutator(&mut self) -> BodyIrDbMutator<'_> {
        BodyIrDbMutator { db: self }
    }

    pub fn stats(&self) -> BodyIrStats {
        let mut stats = BodyIrStats::default();

        for entry in self.packages.raw_entries() {
            let Some(package) = entry.as_resident() else {
                continue;
            };
            for crate_bodies in package.crates() {
                stats.crate_count += 1;
                match crate_bodies.status() {
                    CrateBodiesStatus::Built => stats.built_crate_count += 1,
                    CrateBodiesStatus::Skipped => stats.skipped_crate_count += 1,
                }
                match crate_bodies.coverage() {
                    CrateBodiesCoverage::Complete => stats.complete_crate_count += 1,
                    CrateBodiesCoverage::Partial => stats.partial_crate_count += 1,
                    CrateBodiesCoverage::Missing => stats.missing_crate_count += 1,
                    CrateBodiesCoverage::SkippedByPolicy => {
                        stats.skipped_by_policy_crate_count += 1;
                    }
                }
                stats.body_count += crate_bodies.bodies().len();
                for body in crate_bodies.bodies() {
                    stats.scope_count += body.scopes().len();
                    stats.binding_count += body.bindings().len();
                    stats.statement_count += body.statements().len();
                    stats.expression_count += body.exprs().len();
                }
            }
        }

        stats
    }

    /// Returns one resident package by package slot.
    pub fn resident_package(&self, package: PackageSlot) -> Option<&PackageBodies> {
        self.packages
            .raw_entry(package)
            .and_then(|entry| entry.as_resident())
    }

    /// Returns retained coverage for one semantic crate without loading its package payload.
    pub fn crate_coverage(&self, crate_ref: CrateRef) -> Option<CrateBodiesCoverage> {
        let entry = self.packages.raw_entry(crate_ref.package)?;
        if let Some(package) = entry.as_resident() {
            return package
                .crate_bodies(crate_ref.crate_id)
                .map(|bodies| bodies.coverage());
        }
        entry.as_offloaded()?.crate_coverage(crate_ref.crate_id)
    }

    /// Returns whether one package slot exists but its Body IR payload is not resident.
    ///
    /// This distinguishes a deliberately lazy slot from an invalid slot; [`Self::resident_package`]
    /// returns `None` for both.
    pub fn package_is_offloaded(&self, package: PackageSlot) -> bool {
        self.packages
            .raw_entry(package)
            .is_some_and(|entry| entry.is_offloaded())
    }

    /// Replace the whole slot so an offloaded summary cannot outlive the payload it described.
    pub fn replace_package(&mut self, package: PackageSlot, bodies: PackageBodies) -> Option<()> {
        self.packages.replace(package, bodies)
    }

    pub fn read_txn<'db>(&'db self, loader: BodyIrLoader<'db>) -> BodyIrReadTxn<'db> {
        BodyIrReadTxn::from_store_entries(
            self.packages
                .raw_entries()
                .map(|entry| (true, entry.resident_arc())),
            loader,
        )
    }

    pub fn read_txn_for_subset<'db>(
        &'db self,
        loader: BodyIrLoader<'db>,
        subset: &PackageSubset,
    ) -> BodyIrReadTxn<'db> {
        debug_assert_eq!(
            subset.raw_len(),
            self.packages.len(),
            "package subset should belong to the same Body IR snapshot",
        );
        BodyIrReadTxn::from_store_entries(
            self.packages
                .raw_entries_with_slots()
                .map(|(package, entry)| (subset.contains(package), entry.resident_arc())),
            loader,
        )
    }

    /// Drop one resident payload while keeping its exact crate coverage in the same package slot.
    ///
    /// Residency application can include a package that is already offloaded. Treat that as a
    /// no-op so its existing coverage is preserved rather than replaced with a reconstructed value.
    pub fn offload_package(&mut self, package: PackageSlot) -> Option<()> {
        if self.package_is_offloaded(package) {
            return Some(());
        }
        let coverage = self.resident_package(package)?.coverage();
        self.packages.offload_with(package, coverage)
    }
}

pub(crate) struct BodyIrDbMutator<'db> {
    db: &'db mut BodyIrDb,
}

impl BodyIrDbMutator<'_> {
    pub(crate) fn replace_package(
        &mut self,
        package: PackageSlot,
        bodies: PackageBodies,
    ) -> Option<()> {
        self.db.replace_package(package, bodies)
    }
}

#[cfg(test)]
mod tests {
    use rg_arena::Arena;
    use rg_def_map::PackageSlot;
    use rg_ir_model::{CrateId, CrateRef};
    use rg_package_store::{PackageEntry, PackageStore};

    use crate::{BodyIrDb, CrateBodies, CrateBodiesCoverage, PackageBodies};

    #[test]
    fn coverage_follows_package_replacement_and_offload() {
        let package = PackageSlot(0);
        let first_crate = CrateRef {
            package,
            crate_id: CrateId(0),
        };
        let second_crate = CrateRef {
            package,
            crate_id: CrateId(1),
        };
        let mut db =
            BodyIrDb::from_package_store(PackageStore::from_entries(vec![PackageEntry::resident(
                PackageBodies::new(vec![
                    CrateBodies::empty(CrateBodiesCoverage::Missing),
                    CrateBodies::empty(CrateBodiesCoverage::SkippedByPolicy),
                ]),
            )]));

        assert_eq!(
            db.crate_coverage(first_crate),
            Some(CrateBodiesCoverage::Missing),
        );
        assert_eq!(
            db.crate_coverage(second_crate),
            Some(CrateBodiesCoverage::SkippedByPolicy),
        );

        db.replace_package(
            package,
            PackageBodies::new(vec![
                CrateBodies::from_build(
                    CrateBodiesCoverage::Complete,
                    Arena::new(),
                    Arena::new(),
                    Arena::new(),
                ),
                CrateBodies::empty(CrateBodiesCoverage::SkippedByPolicy),
            ]),
        )
        .expect("Body IR package slot should exist");
        assert_eq!(
            db.crate_coverage(first_crate),
            Some(CrateBodiesCoverage::Complete),
        );

        db.offload_package(package)
            .expect("Body IR package slot should exist");
        assert!(db.resident_package(package).is_none());
        assert_eq!(
            db.crate_coverage(first_crate),
            Some(CrateBodiesCoverage::Complete),
        );
        assert_eq!(
            db.crate_coverage(second_crate),
            Some(CrateBodiesCoverage::SkippedByPolicy),
        );
    }
}
