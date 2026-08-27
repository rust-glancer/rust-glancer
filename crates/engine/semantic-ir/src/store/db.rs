//! Semantic IR package store and transaction entry points.

use std::collections::HashMap;

use rg_def_map::PackageSlot;
use rg_ir_model::{ImplRef, TraitDefRef, TypeDefRef};
use rg_package_store::{PackageLoader, PackageStore, PackageSubset};
use rg_std::{ExpectedUnique, MemorySize, Shrink};

use crate::{PackageIr, SemanticIrReadTxn, SemanticIrStats, TraitImplSelfHead};

/// Semantic item graph for all analyzed packages and semantic crates.
///
/// Semantic IR is the signature layer: it keeps named items, fields, impl headers, function
/// signatures, and enough resolution metadata to answer LSP-shaped questions without parsing AST
/// again. Bodies live in `rg_body_ir`; this layer intentionally stops at item/signature facts.
#[derive(Debug, Clone, PartialEq, Eq, Default, MemorySize)]
pub struct SemanticIrDb {
    packages: PackageStore<PackageIr>,
}

impl SemanticIrDb {
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

    /// Returns coarse item counts for status output and smoke checks.
    pub fn stats(&self) -> SemanticIrStats {
        let mut stats = SemanticIrStats::default();

        for entry in self.packages.raw_entries() {
            let Some(package) = entry.as_resident() else {
                continue;
            };
            for (crate_idx, items) in package.crates().iter().enumerate() {
                stats.crate_count += 1;
                stats.struct_count += items.structs().len();
                stats.union_count += items.unions().len();
                stats.enum_count += items.enums().len();
                stats.trait_count += items.traits().len();
                stats.impl_count += items.impls().len();
                stats.function_count += items.functions().len();
                stats.type_alias_count += items.type_aliases().len();
                stats.const_count += items.consts().len();
                stats.static_count += items.statics().len();
                if let Some(index) = package.crate_lookup_index(rg_ir_model::CrateId(crate_idx)) {
                    stats.lookup_index_count += 1;
                    stats.lookup_index_entry_count += index.entry_count();
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
    pub fn resident_package(&self, package: PackageSlot) -> Option<&PackageIr> {
        self.packages
            .raw_entry(package)
            .and_then(|entry| entry.as_resident())
    }

    /// Replaces one package payload while preserving the surrounding package-store shape.
    ///
    /// Exact on-demand Body IR rebuilds use this to temporarily restore an artifact-backed
    /// package. Rewriting that artifact requires every phase payload to be resident together.
    pub fn replace_package(&mut self, package: PackageSlot, package_ir: PackageIr) -> Option<()> {
        self.packages.replace(package, package_ir)
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
    pub(crate) fn replace_package(
        &mut self,
        package: PackageSlot,
        package_ir: PackageIr,
    ) -> Option<()> {
        self.db.replace_package(package, package_ir)
    }

    pub(crate) fn set_impl_header_facts(
        &mut self,
        impl_ref: ImplRef,
        resolved_self_ty: ExpectedUnique<TypeDefRef>,
        resolved_trait_ref: ExpectedUnique<TraitDefRef>,
    ) -> Option<()> {
        let crate_ref = impl_ref.origin.as_crate_ref()?;
        self.package_mut(crate_ref.package)?
            .crate_items_mut(crate_ref.crate_id)?
            .set_impl_header_facts(impl_ref.id, resolved_self_ty, resolved_trait_ref)
    }

    fn package_mut(&mut self, package: PackageSlot) -> Option<&mut PackageIr> {
        self.db.packages.make_mut(package)
    }

    pub(crate) fn compact_packages(&mut self, packages: &[PackageSlot]) {
        for package in packages {
            if let Some(package) = self.db.packages.get_unique_mut(*package) {
                Shrink::shrink_to_fit(package);
            }
        }
    }

    pub(crate) fn rebuild_lookup_indexes(
        &mut self,
        packages: &[PackageSlot],
        self_heads: &HashMap<ImplRef, TraitImplSelfHead>,
    ) {
        for package in packages {
            if let Some(package) = self.db.packages.make_mut(*package) {
                package.rebuild_lookup_indexes(self_heads);
            }
        }
    }
}
