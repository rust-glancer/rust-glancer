//! Semantic IR package store and transaction entry points.

use rg_def_map::{Package as DefMapPackage, PackageSlot};
use rg_memsize::{MemoryRecorder, MemorySize};
use rg_package_store::{PackageLoader, PackageStore, PackageSubset};

use crate::{
    ImplData, ImplRef, PackageIr, SemanticIrReadTxn, SemanticIrStats,
    build::{SemanticIrDbBuilder, SemanticIrDbPackageRebuilder},
};

/// Semantic item graph for all analyzed packages and targets.
///
/// Semantic IR is the signature layer: it keeps named items, fields, impl headers, function
/// signatures, and enough resolution metadata to answer LSP-shaped questions without parsing AST
/// again. Bodies live in `rg_body_ir`; this layer intentionally stops at item/signature facts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SemanticIrDb {
    packages: PackageStore<PackageIr>,
}

impl SemanticIrDb {
    /// Starts building semantic IR from collected item trees and frozen name-resolution maps.
    pub fn builder<'db>(
        item_tree: &'db rg_item_tree::ItemTreeDb,
        def_map: &'db rg_def_map::DefMapDb,
    ) -> SemanticIrDbBuilder<'db> {
        SemanticIrDbBuilder::new(item_tree, def_map)
    }

    /// Starts rebuilding selected packages against lazy read views.
    pub fn package_rebuilder<'db>(
        &'db self,
        item_tree: &'db rg_item_tree::ItemTreeDb,
        def_map: &'db rg_def_map::DefMapDb,
        packages: &'db [PackageSlot],
        def_map_loader: PackageLoader<'db, DefMapPackage>,
        semantic_ir_loader: PackageLoader<'db, PackageIr>,
        subset: &'db PackageSubset,
    ) -> SemanticIrDbPackageRebuilder<'db> {
        SemanticIrDbPackageRebuilder::new(
            self,
            item_tree,
            def_map,
            packages,
            def_map_loader,
            semantic_ir_loader,
            subset,
        )
    }

    pub(crate) fn from_packages(packages: Vec<PackageIr>) -> Self {
        Self::from_package_store(PackageStore::from_vec(packages))
    }

    /// Builds a semantic IR database from an already shaped package store.
    ///
    /// This keeps cache-loading code from reaching into the database internals while still letting
    /// it preserve the same resident/offloaded slot layout used by normal package residency.
    pub fn from_package_store(packages: PackageStore<PackageIr>) -> Self {
        Self { packages }
    }

    pub(crate) fn mutator(&mut self) -> SemanticIrDbMutator<'_> {
        SemanticIrDbMutator { db: self }
    }

    pub(crate) fn record_packages_memory_children(&self, recorder: &mut MemoryRecorder) {
        self.packages.record_memory_children(recorder);
    }

    /// Returns coarse item counts for status output and smoke checks.
    pub fn stats(&self) -> SemanticIrStats {
        let mut stats = SemanticIrStats::default();

        for entry in self.packages.raw_entries() {
            let Some(package) = entry.as_resident() else {
                continue;
            };
            for target in package.targets() {
                let items = target.items();
                stats.target_count += 1;
                stats.struct_count += items.structs.len();
                stats.union_count += items.unions.len();
                stats.enum_count += items.enums.len();
                stats.trait_count += items.traits.len();
                stats.impl_count += items.impls.len();
                stats.function_count += items.functions.len();
                stats.type_alias_count += items.type_aliases.len();
                stats.const_count += items.consts.len();
                stats.static_count += items.statics.len();
            }
        }

        stats
    }

    /// Returns the number of package slots tracked by this snapshot.
    pub fn package_count(&self) -> usize {
        self.packages.len()
    }

    /// Returns one resident package by package slot.
    pub fn resident_package(&self, package: PackageSlot) -> Option<&PackageIr> {
        self.packages
            .raw_entry(package)
            .and_then(|entry| entry.as_resident())
    }

    pub fn read_txn<'db>(
        &'db self,
        loader: PackageLoader<'db, PackageIr>,
    ) -> SemanticIrReadTxn<'db> {
        SemanticIrReadTxn::from_package_store(self.packages.read_txn(loader))
    }

    pub fn read_txn_for_subset<'db>(
        &'db self,
        loader: PackageLoader<'db, PackageIr>,
        subset: &PackageSubset,
    ) -> SemanticIrReadTxn<'db> {
        SemanticIrReadTxn::from_package_store(self.packages.read_txn_for_subset(loader, subset))
    }

    pub fn offload_package(&mut self, package: PackageSlot) -> Option<()> {
        self.packages.offload(package)
    }
}

pub(crate) struct SemanticIrDbMutator<'db> {
    db: &'db mut SemanticIrDb,
}

impl SemanticIrDbMutator<'_> {
    pub(crate) fn package_count(&self) -> usize {
        self.db.package_count()
    }

    pub(crate) fn read_txn<'a>(
        &'a self,
        loader: PackageLoader<'a, PackageIr>,
    ) -> SemanticIrReadTxn<'a> {
        self.db.read_txn(loader)
    }

    pub(crate) fn replace_package(
        &mut self,
        package: PackageSlot,
        package_ir: PackageIr,
    ) -> Option<()> {
        self.db.packages.replace(package, package_ir)
    }

    pub(crate) fn impl_data_mut(&mut self, impl_ref: ImplRef) -> Option<&mut ImplData> {
        self.package_mut(impl_ref.target.package)?
            .target_mut(impl_ref.target.target)?
            .items_mut()
            .impls
            .get_mut(impl_ref.id)
    }

    fn package_mut(&mut self, package: PackageSlot) -> Option<&mut PackageIr> {
        self.db.packages.make_mut(package)
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
