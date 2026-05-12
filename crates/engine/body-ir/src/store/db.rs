//! Body IR package store and transaction entry points.

use rg_def_map::{Package as DefMapPackage, PackageSlot};
use rg_memsize::{MemoryRecorder, MemorySize};
use rg_package_store::{PackageLoader, PackageStore, PackageSubset};
use rg_semantic_ir::PackageIr;
use rg_text::PackageNameInterners;

use crate::{
    BodyIrReadTxn, BodyIrStats, PackageBodies, TargetBodiesStatus,
    build::{BodyIrDbBuilder, BodyIrDbPackageRebuilder},
};

/// Body-level IR for all analyzed packages and targets.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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

    pub(crate) fn record_packages_memory_children(&self, recorder: &mut MemoryRecorder) {
        self.packages.record_memory_children(recorder);
    }

    pub fn stats(&self) -> BodyIrStats {
        let mut stats = BodyIrStats::default();

        for entry in self.packages.raw_entries() {
            let Some(package) = entry.as_resident() else {
                continue;
            };
            for target in package.targets() {
                stats.target_count += 1;
                match target.status() {
                    TargetBodiesStatus::Built => stats.built_target_count += 1,
                    TargetBodiesStatus::Skipped => stats.skipped_target_count += 1,
                }
                stats.body_count += target.bodies().len();
                for body in target.bodies() {
                    stats.scope_count += body.scopes.len();
                    stats.local_item_count += body.local_items.len();
                    stats.local_impl_count += body.local_impls.len();
                    stats.local_function_count += body.local_functions.len();
                    stats.binding_count += body.bindings.len();
                    stats.statement_count += body.statements.len();
                    stats.expression_count += body.exprs.len();
                }
            }
        }

        stats
    }

    /// Returns the number of package slots tracked by this snapshot.
    pub fn package_count(&self) -> usize {
        self.packages.len()
    }

    /// Returns one resident package by package slot.
    pub fn resident_package(&self, package: PackageSlot) -> Option<&PackageBodies> {
        self.packages
            .raw_entry(package)
            .and_then(|entry| entry.as_resident())
    }

    pub fn read_txn<'db>(
        &'db self,
        loader: PackageLoader<'db, PackageBodies>,
    ) -> BodyIrReadTxn<'db> {
        BodyIrReadTxn::from_package_store(self.packages.read_txn(loader))
    }

    pub fn read_txn_for_subset<'db>(
        &'db self,
        loader: PackageLoader<'db, PackageBodies>,
        subset: &PackageSubset,
    ) -> BodyIrReadTxn<'db> {
        BodyIrReadTxn::from_package_store(self.packages.read_txn_for_subset(loader, subset))
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
