//! Body IR package store and transaction entry points.

use rg_def_map::PackageDefMaps as DefMapPackage;
use rg_def_map::PackageSlot;
use rg_package_store::{PackageLoader, PackageStore, PackageSubset};
use rg_semantic_ir::PackageIr;
use rg_text::PackageNameInterners;

use super::{BodyIrLoader, BodyIrReadTxn, CrateBodiesCoverage, CrateBodiesStatus, PackageBodies};
use crate::build::{BodyIrDbBuilder, BodyIrDbPackageRebuilder};
use rg_std::{MemorySize, Shrink};

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
#[derive(Debug, Clone, PartialEq, Eq, Default, MemorySize)]
pub struct BodyIrDb {
    packages: PackageStore<PackageBodies>,
}

impl BodyIrDb {
    /// Starts building Body IR.
    pub fn builder<'db>(
        parse: &'db rg_parse::ParseDb,
        def_map: &'db rg_def_map::DefMapDb,
        semantic_ir: &'db rg_semantic_ir::SemanticIrDb,
    ) -> BodyIrDbBuilder<'db, 'static> {
        BodyIrDbBuilder::new(parse, def_map, semantic_ir)
    }

    /// Starts rebuilding selected packages against lazy read views.
    #[allow(clippy::too_many_arguments)]
    pub fn package_rebuilder<'db, 'names>(
        &'db self,
        parse: &'db rg_parse::ParseDb,
        def_map: &'db rg_def_map::DefMapDb,
        semantic_ir: &'db rg_semantic_ir::SemanticIrDb,
        packages: &'db [PackageSlot],
        interners: &'names mut PackageNameInterners,
        def_map_loader: PackageLoader<'db, DefMapPackage>,
        semantic_ir_loader: PackageLoader<'db, PackageIr>,
        subset: &'db PackageSubset,
    ) -> BodyIrDbPackageRebuilder<'db, 'names> {
        BodyIrDbPackageRebuilder::new(
            self,
            parse,
            def_map,
            semantic_ir,
            packages,
            interners,
            def_map_loader,
            semantic_ir_loader,
            subset,
        )
    }

    pub(crate) fn from_packages(packages: Vec<PackageBodies>) -> Self {
        Self::from_package_store(PackageStore::from_vec(packages))
    }

    /// Builds a Body IR database from an already shaped package store.
    ///
    /// Startup cache loading will validate package artifacts before deciding which slots stay
    /// resident and which slots remain lazy; the database should only need to own that final store.
    pub fn from_package_store(packages: PackageStore<PackageBodies>) -> Self {
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

    /// Returns whether one package slot exists but its Body IR payload is not resident.
    ///
    /// This distinguishes a deliberately lazy slot from an invalid slot; [`Self::resident_package`]
    /// returns `None` for both.
    pub fn package_is_offloaded(&self, package: PackageSlot) -> bool {
        self.packages
            .raw_entry(package)
            .is_some_and(|entry| entry.is_offloaded())
    }

    /// Replaces one package payload while preserving the surrounding package-store shape.
    ///
    /// This is used by project-level monotonic Body IR merges, where a background completion
    /// contributes a package that has strictly better coverage than the current saved project.
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

    pub fn offload_package(&mut self, package: PackageSlot) -> Option<()> {
        self.packages.offload(package)
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
        self.db.packages.replace(package, bodies)
    }

    pub(crate) fn compact_storage(&mut self) {
        Shrink::shrink_to_fit(&mut self.db.packages);
    }
}
