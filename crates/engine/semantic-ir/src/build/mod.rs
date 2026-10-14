//! Builds semantic item stores and rebuilds semantic IR package snapshots.

mod impl_headers;
mod item_store;
mod package;

use std::collections::HashMap;

use anyhow::Context as _;
use rg_def_map::{DefMapLoader, GeneratedItemStores};
use rg_ir_model::{CrateId, CrateRef, PackageSlot};
use rg_package_store::PackageSubset;

pub use self::item_store::{ItemStoreLowerer, ItemStoreSourceReader};
use crate::{CrateItemQuery, ItemLookupFacts, SemanticIrDb, SemanticIrLoader};

impl SemanticIrDb {
    /// Builds selected Semantic IR packages on top of this snapshot.
    ///
    /// Fresh construction starts from an all-offloaded snapshot, while saved updates start from the
    /// previous project snapshot. Both cases use the same package replacement and lazy-read protocol.
    ///
    /// The selection must include affected reverse dependents: their saved lookup facts can point
    /// to item ids in a changed dependency even when their own source has not changed.
    #[allow(clippy::too_many_arguments)]
    pub fn build_packages<'db>(
        &'db self,
        item_tree: &'db rg_item_tree::ItemTreeDb,
        def_map: &'db rg_def_map::DefMapDb,
        generated_items: &'db GeneratedItemStores,
        packages: &'db [PackageSlot],
        def_map_loader: DefMapLoader<'db>,
        semantic_ir_loader: SemanticIrLoader<'db>,
        subset: &'db PackageSubset,
    ) -> anyhow::Result<Self> {
        let mut next = self.clone();
        let packages = normalized_package_slots(packages);

        {
            let mut mutator = next.mutator();
            for package in &packages {
                let rebuilt =
                    package::build_package(item_tree, def_map, generated_items, *package)?;
                mutator
                    .replace_package(*package, rebuilt)
                    .with_context(|| {
                        format!(
                            "while attempting to replace semantic IR package {}",
                            package.0
                        )
                    })?;
            }
        }

        let def_map_txn = def_map.read_txn_for_subset(def_map_loader, subset);
        let semantic_ir_txn = next.read_txn_for_subset(semantic_ir_loader, subset);
        let impl_resolutions = impl_headers::impl_header_resolutions_for_packages(
            &semantic_ir_txn,
            &def_map_txn,
            &packages,
        )
        .context("while attempting to resolve selected semantic IR impl headers")?;

        // Record each rebuilt crate's visibility and language-item choices. All selected
        // declaration stores have been replaced, so this reads the new declarations even when
        // several dependent packages are rebuilt together. Save the facts with the candidate
        // tables below so queries do not have to reconstruct them.
        let cancellation = rg_std::CancellationToken::new();
        let mut lookup_facts = HashMap::new();
        for package in &packages {
            let crate_count = semantic_ir_txn.package(*package)?.crates().len();
            for crate_idx in 0..crate_count {
                let crate_ref = CrateRef {
                    package: *package,
                    crate_id: CrateId(crate_idx),
                };
                let facts = ItemLookupFacts::build(
                    &CrateItemQuery::new(&def_map_txn, &semantic_ir_txn, crate_ref),
                    &cancellation,
                )?;
                lookup_facts.insert(crate_ref, facts);
            }
        }
        drop(semantic_ir_txn);

        {
            // Candidate lookup keys depend on the resolved impl headers, so apply those headers
            // before constructing the indexes that carry both candidates and saved lookup facts.
            let mut mutator = next.mutator();
            let self_heads =
                impl_headers::apply_impl_header_resolutions(&mut mutator, impl_resolutions);
            mutator.rebuild_lookup_indexes(&packages, &self_heads, &lookup_facts);
            mutator.shrink_packages_to_fit(&packages);
        }

        Ok(next)
    }
}

fn normalized_package_slots(packages: &[PackageSlot]) -> Vec<PackageSlot> {
    let mut slots = packages.to_vec();
    slots.sort_by_key(|slot| slot.0);
    slots.dedup();
    slots
}

fn unexpected_package_loader() -> DefMapLoader<'static> {
    DefMapLoader::resident_only("resident semantic IR build")
}
