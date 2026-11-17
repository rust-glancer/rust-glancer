//! Builds semantic packages from resolved crate declarations.
//!
//! The def-map owns name-resolution identity, while the item tree owns syntax-shaped declarations.
//! This pass joins those two views into stable semantic items that later query layers can use
//! without walking AST or module scopes again.

use anyhow::Context as _;
use rg_def_map::{DefMap, GeneratedItemStore, ItemSource, ItemSourceKind};
use rg_item_tree::{ItemNode, ItemTrees};

use super::{ItemStoreLowerer, ItemStoreSourceReader};
use crate::ItemStore;

struct CrateLowering<'a> {
    item_tree: &'a ItemTrees,
    generated_items: &'a GeneratedItemStore,
}

impl<'a> ItemStoreSourceReader<'a> for CrateLowering<'a> {
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
            ItemSourceKind::Generated(item_ref) | ItemSourceKind::Synthetic(item_ref) => {
                self.generated_items.item(item_ref).with_context(|| {
                    format!(
                        "while attempting to find generated item {:?} from generated source {:?}",
                        item_ref.item, item_ref.source
                    )
                })?
            }
            ItemSourceKind::Body(_) => anyhow::bail!("Body is not supported"),
        };
        Ok(item)
    }

    fn associated_macro_expansion(&self, source: ItemSource) -> Option<&[ItemSource]> {
        self.generated_items.associated_macro_expansion(source)
    }
}

impl ItemStore {
    /// Lower this crate's declarations in DefMap identity order while its generated items
    /// are still available from construction.
    pub(crate) fn lower_crate(
        definitions: &DefMap,
        item_tree: &ItemTrees,
        generated_items: &GeneratedItemStore,
    ) -> anyhow::Result<Self> {
        ItemStoreLowerer::new(
            definitions,
            CrateLowering {
                item_tree,
                generated_items,
            },
        )
        .lower()
    }
}
