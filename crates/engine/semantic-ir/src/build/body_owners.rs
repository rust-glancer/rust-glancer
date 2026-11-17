use rg_def_map::{DefMap, ItemSource};
use rg_ir_model::{
    FileId, ItemOwner, ItemStoreRef, ModuleRef, OuterBodyOwner, SemanticItemRef, SourceId,
};
use wincode::{SchemaRead, SchemaWrite};

use crate::{AssociatedItemStore, ItemStore, SemanticItemView};

/// Where to open the source of a target-level body before any Body IR exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct BodyOwnerData {
    pub owner: OuterBodyOwner,
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

    /// A declaration without a source body should not schedule inference, even when it has a
    /// semantic identity. Module statics also need their foreign-block context checked here.
    pub fn for_crate<'a>(
        items: &'a ItemStore,
        definitions: &'a DefMap,
    ) -> impl Iterator<Item = Self> + 'a {
        let declarations = items.scope_items().filter(|item| {
            item.static_data().is_none_or(|data| {
                definitions
                    .foreign_block(data.local_def.local_def)
                    .is_none()
            })
        });
        Self::for_items(
            |owner| match owner {
                ItemOwner::Module(module) => Some(module),
                ItemOwner::Trait(id) => items.trait_data(id).map(|data| data.owner),
                ItemOwner::Impl(id) => items.impl_data(id).map(|data| data.owner),
            },
            declarations,
        )
    }

    pub(crate) fn for_members<'a>(
        module: ModuleRef,
        members: &'a AssociatedItemStore,
        store: ItemStoreRef,
    ) -> impl Iterator<Item = Self> + 'a {
        // Every member inherits its module from the selected owner header. No other headers
        // or module definitions are needed; associated stores contain no foreign statics.
        Self::for_items(move |_| Some(module), members.semantic_items(store))
    }

    fn for_items<'a>(
        module_for: impl Fn(ItemOwner) -> Option<ModuleRef> + 'a,
        declarations: impl Iterator<Item = SemanticItemView<'a>> + 'a,
    ) -> impl Iterator<Item = Self> + 'a {
        declarations.filter_map(move |item| {
            let (owner, module, location) = match item.item() {
                SemanticItemRef::Function(reference) => {
                    let data = item.function_data()?;
                    if !data.signature.has_body() {
                        return None;
                    }
                    (
                        OuterBodyOwner::from(reference),
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
                        OuterBodyOwner::from(reference),
                        module_for(data.owner)?,
                        data.location,
                    )
                }
                SemanticItemRef::Static(reference) => {
                    let data = item.static_data()?;
                    (OuterBodyOwner::from(reference), data.owner, data.location)
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
