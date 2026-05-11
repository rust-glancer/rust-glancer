//! Builds and rebuilds semantic IR snapshots.

mod impl_headers;
mod lower;

use std::{fmt, marker::PhantomData, sync::Arc};

use anyhow::Context as _;

use rg_def_map::{Package as DefMapPackage, PackageSlot};
use rg_package_store::{LoadPackage, PackageLoader, PackageStoreError, PackageSubset};

use crate::{PackageIr, SemanticIrDb};

/// Builder for a fresh semantic IR snapshot.
pub struct SemanticIrDbBuilder<'db> {
    item_tree: &'db rg_item_tree::ItemTreeDb,
    def_map: &'db rg_def_map::DefMapDb,
}

impl<'db> SemanticIrDbBuilder<'db> {
    pub(crate) fn new(
        item_tree: &'db rg_item_tree::ItemTreeDb,
        def_map: &'db rg_def_map::DefMapDb,
    ) -> Self {
        Self { item_tree, def_map }
    }

    pub fn build(self) -> anyhow::Result<SemanticIrDb> {
        let packages = lower::build_packages(self.item_tree, self.def_map)?;
        let mut db = SemanticIrDb::from_packages(packages);
        {
            let mut mutator = db.mutator();
            impl_headers::resolve_impl_headers(&mut mutator, self.def_map)
                .context("while attempting to resolve semantic IR impl headers")?;
            mutator.shrink_to_fit();
        }
        Ok(db)
    }
}

/// Builder for a semantic IR snapshot that replaces selected packages.
pub struct SemanticIrDbPackageRebuilder<'db> {
    old: &'db SemanticIrDb,
    item_tree: &'db rg_item_tree::ItemTreeDb,
    def_map: &'db rg_def_map::DefMapDb,
    packages: &'db [PackageSlot],
    def_map_loader: PackageLoader<'db, DefMapPackage>,
    semantic_ir_loader: PackageLoader<'db, PackageIr>,
    subset: &'db PackageSubset,
}

impl<'db> SemanticIrDbPackageRebuilder<'db> {
    pub(crate) fn new(
        old: &'db SemanticIrDb,
        item_tree: &'db rg_item_tree::ItemTreeDb,
        def_map: &'db rg_def_map::DefMapDb,
        packages: &'db [PackageSlot],
        def_map_loader: PackageLoader<'db, DefMapPackage>,
        semantic_ir_loader: PackageLoader<'db, PackageIr>,
        subset: &'db PackageSubset,
    ) -> SemanticIrDbPackageRebuilder<'db> {
        Self {
            old,
            item_tree,
            def_map,
            packages,
            def_map_loader,
            semantic_ir_loader,
            subset,
        }
    }

    pub fn build(self) -> anyhow::Result<SemanticIrDb> {
        let mut next = self.old.clone();
        let packages = normalized_package_slots(self.packages);

        {
            let mut mutator = next.mutator();
            for package in &packages {
                let rebuilt = lower::build_package(self.item_tree, self.def_map, *package)?;
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

        let def_map_txn = self
            .def_map
            .read_txn_for_subset(self.def_map_loader, self.subset);
        let semantic_ir_txn = next.read_txn_for_subset(self.semantic_ir_loader, self.subset);
        let impl_resolutions = impl_headers::impl_header_resolutions_for_packages(
            &semantic_ir_txn,
            &def_map_txn,
            &packages,
        )
        .context("while attempting to resolve rebuilt semantic IR impl headers")?;
        drop(semantic_ir_txn);

        {
            let mut mutator = next.mutator();
            impl_headers::apply_impl_header_resolutions(&mut mutator, impl_resolutions);
            mutator.shrink_packages(&packages);
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

fn unexpected_package_loader<T: 'static>() -> PackageLoader<'static, T> {
    PackageLoader::new(UnexpectedPackageLoader(PhantomData))
}

struct UnexpectedPackageLoader<T>(PhantomData<fn() -> T>);

impl<T> fmt::Debug for UnexpectedPackageLoader<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnexpectedPackageLoader").finish()
    }
}

impl<T> LoadPackage<T> for UnexpectedPackageLoader<T> {
    fn load(&self, package: PackageSlot) -> Result<Arc<T>, PackageStoreError> {
        panic!(
            "resident semantic IR build should not load offloaded package {}",
            package.0,
        )
    }
}
