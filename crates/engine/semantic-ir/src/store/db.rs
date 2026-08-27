//! Semantic IR package store and transaction entry points.

use std::collections::HashMap;

use rg_def_map::PackageSlot;
use rg_ir_model::{ImplRef, TraitDefRef, TypeDefRef};
use rg_package_store::{PackageStore, PackageSubset};
use rg_std::{ExpectedUnique, MemorySize, Shrink};

use crate::{PackageIr, SemanticIrLoader, SemanticIrReadTxn, SemanticIrStats, TraitImplSelfHead};

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
            for (crate_idx, crate_ir) in package.crates().iter().enumerate() {
                let items = crate_ir.items();
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

    pub fn package_is_offloaded(&self, package: PackageSlot) -> bool {
        self.packages
            .raw_entry(package)
            .is_some_and(|entry| entry.is_offloaded())
    }

    /// Opens a read transaction over every package slot in this snapshot.
    ///
    /// Resident packages are borrowed directly. Exact item or lookup-index reads from offloaded
    /// packages go through the supplied loader and remain request-local.
    pub fn read_txn<'db>(&'db self, loader: SemanticIrLoader<'db>) -> SemanticIrReadTxn<'db> {
        SemanticIrReadTxn::from_store_entries(
            self.packages
                .raw_entries()
                .map(|entry| (true, entry.resident_arc())),
            loader,
        )
    }

    /// Opens a read transaction that rejects packages outside `subset`.
    ///
    /// Keeping excluded slots in place preserves project-wide package references while preventing
    /// a scoped query from loading unrelated artifacts.
    pub fn read_txn_for_subset<'db>(
        &'db self,
        loader: SemanticIrLoader<'db>,
        subset: &PackageSubset,
    ) -> SemanticIrReadTxn<'db> {
        debug_assert_eq!(
            subset.raw_len(),
            self.packages.len(),
            "package subset should belong to the same Semantic IR snapshot",
        );
        SemanticIrReadTxn::from_store_entries(
            self.packages
                .raw_entries_with_slots()
                .map(|(package, entry)| (subset.contains(package), entry.resident_arc())),
            loader,
        )
    }

    /// Releases a resident package after its independently readable artifact has been written.
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
        self.db.packages.replace(package, package_ir)
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
