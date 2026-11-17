use rg_def_map::{DefMap, ItemSource};
use rg_ir_model::{
    FileId, ItemOwner, ItemStoreRef, ModuleRef, SavedBodyOwner, SemanticItemRef, SourceId,
};
use wincode::{SchemaRead, SchemaWrite};

use crate::ItemStore;

/// Where to open the source of a saved target-level body before any Body IR exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct BodyOwnerData {
    pub owner: SavedBodyOwner,
    pub module: ModuleRef,
    pub file: FileId,
    pub location: SourceId,
    pub source: ItemSource,
}

impl BodyOwnerData {
    pub fn store_ref(self) -> ItemStoreRef {
        ItemStoreRef {
            origin: self.module.origin,
            id: self.owner.store(),
        }
    }

    /// Enumerate scope declarations followed by member arenas. A declaration without a source body
    /// should not schedule inference, even when it has a semantic identity.
    pub fn for_crate<'a>(
        items: &'a ItemStore,
        definitions: &'a DefMap,
    ) -> impl Iterator<Item = Self> + 'a {
        let module_for = |owner| match owner {
            ItemOwner::Module(module) => Some(module),
            ItemOwner::Trait(id) => items.trait_data(id).map(|data| data.owner),
            ItemOwner::Impl(id) => items.impl_data(id).map(|data| data.owner),
        };
        items.semantic_items().filter_map(move |item| {
            let (owner, module, location) = match item.item() {
                SemanticItemRef::Function(reference) => {
                    let data = item.function_data()?;
                    if !data.signature.has_body() {
                        return None;
                    }
                    (
                        SavedBodyOwner::from(reference),
                        module_for(data.owner)?,
                        data.location,
                    )
                }
                SemanticItemRef::Const(reference) => {
                    let data = item.const_data()?;
                    if !data.signature.has_value() {
                        return None;
                    }
                    (
                        SavedBodyOwner::from(reference),
                        module_for(data.owner)?,
                        data.location,
                    )
                }
                SemanticItemRef::Static(reference) => {
                    let data = item.static_data()?;
                    // Extern statics have a declaration but no initializer to lower.
                    if definitions
                        .foreign_block(data.local_def.local_def)
                        .is_some()
                    {
                        return None;
                    }
                    (SavedBodyOwner::from(reference), data.owner, data.location)
                }
                _ => return None,
            };
            Some(Self {
                owner,
                module,
                file: item.source().file_id,
                location,
                source: item.source(),
            })
        })
    }
}
