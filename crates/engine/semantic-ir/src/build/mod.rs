//! Builds and rebuilds semantic IR snapshots.

mod impl_headers;
mod lower;

use anyhow::Context as _;

use rg_def_map::{DefMapLoader, GeneratedItemStores, PackageSlot};
use rg_package_store::PackageSubset;

use crate::{SemanticIrDb, SemanticIrLoader};

impl SemanticIrDb {
    /// Builds selected Semantic IR packages on top of this snapshot.
    ///
    /// Fresh construction starts from an all-offloaded snapshot, while saved updates start from the
    /// previous project snapshot. Both cases use the same package replacement and lazy-read protocol.
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
                let rebuilt = lower::build_package(item_tree, def_map, generated_items, *package)?;
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
        drop(semantic_ir_txn);

        {
            let mut mutator = next.mutator();
            let self_heads =
                impl_headers::apply_impl_header_resolutions(&mut mutator, impl_resolutions);
            mutator.rebuild_lookup_indexes(&packages, &self_heads);
            mutator.compact_packages(&packages);
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
