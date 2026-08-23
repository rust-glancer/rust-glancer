//! Lowers resolved module items into the semantic signature graph.
//!
//! The def-map owns name-resolution identity, while the item tree owns syntax-shaped declarations.
//! This pass joins those two views into stable semantic items that later query layers can use
//! without walking AST or module scopes again.

use anyhow::Context as _;

use crate::ItemStore;
use rg_def_map::{DefMapDb, DefMapReadTxn, ItemSource, ItemSourceKind, PackageSlot};
use rg_ir_model::{CrateId, CrateRef};
use rg_item_tree::{ItemNode, ItemTreeDb, Package as ItemTreePackage};

use crate::{ItemStoreLowerer, ItemStoreSourceReader, PackageIr};

pub(super) fn build_package(
    item_tree: &ItemTreeDb,
    def_map: &DefMapDb,
    package: PackageSlot,
) -> anyhow::Result<PackageIr> {
    let def_map_package = def_map
        .resident_package(package)
        .with_context(|| format!("while attempting to fetch def-map package {}", package.0))?;
    let item_tree_package = item_tree
        .package(package.0)
        .with_context(|| format!("while attempting to fetch item tree package {}", package.0))?;
    let mut crates = Vec::with_capacity(def_map_package.crates().len());
    let def_map_txn = def_map.read_txn(super::unexpected_package_loader());

    for (crate_idx, _) in def_map_package.crates().iter().enumerate() {
        let crate_ref = CrateRef {
            package,
            crate_id: CrateId(crate_idx),
        };
        crates.push(
            CrateLowering::new(item_tree_package, crate_ref, &def_map_txn)
                .lower()
                .with_context(|| {
                    format!("while attempting to lower semantic IR for crate {crate_idx}")
                })?,
        );
    }

    Ok(PackageIr::new(crates))
}

struct CrateLowering<'a, 'db> {
    item_tree: &'a ItemTreePackage,
    crate_ref: CrateRef,
    def_map_txn: &'a DefMapReadTxn<'db>,
}

impl<'a, 'db> CrateLowering<'a, 'db> {
    fn new(
        item_tree: &'a ItemTreePackage,
        crate_ref: CrateRef,
        def_map_txn: &'a DefMapReadTxn<'db>,
    ) -> Self {
        Self {
            item_tree,
            crate_ref,
            def_map_txn,
        }
    }

    fn lower(self) -> anyhow::Result<ItemStore> {
        // Local definitions already come from the def-map, so lowering follows def-map identity
        // order and only asks the item tree for declaration payloads.
        let def_map = self
            .def_map_txn
            .def_map(self.crate_ref)
            .with_context(|| {
                format!(
                    "while attempting to fetch def-map local definitions for crate {:?}",
                    self.crate_ref.crate_id,
                )
            })?
            .context("No defmap to lower from")?;
        ItemStoreLowerer::new(def_map, self).lower()
    }
}

impl<'a, 'db> ItemStoreSourceReader<'a> for CrateLowering<'a, 'db> {
    fn item(&self, source: ItemSource) -> anyhow::Result<&'a ItemNode> {
        let item = match source.kind {
            ItemSourceKind::ItemTree(item_ref) => {
                self.item_tree.item(item_ref).with_context(|| {
                    format!(
                        "while attempting to fetch item-tree node {:?} in {:?}",
                        item_ref.item, item_ref.file_id
                    )
                })?
            }
            ItemSourceKind::Generated(item_ref) => self
                .def_map_txn
                .def_map(self.crate_ref)
                .with_context(|| {
                    format!(
                        "while attempting to fetch generated item {:?} from generated source {:?}",
                        item_ref.item, item_ref.source
                    )
                })?
                .and_then(|def_map| def_map.generated_source(item_ref.source))
                .and_then(|source| source.item(item_ref.item))
                .with_context(|| {
                    format!(
                        "while attempting to find generated item {:?} from generated source {:?}",
                        item_ref.item, item_ref.source
                    )
                })?,
            ItemSourceKind::Body(_) => anyhow::bail!("Body is not supported"),
        };
        Ok(item)
    }
}
