use rg_def_map::DefMap;
use rg_ir_model::{FileId, ItemOwner, ModuleRef, SavedBodyOwner, Span};
use wincode::{SchemaRead, SchemaWrite};

use crate::ItemStore;

/// Where to open the source of a saved target-level body before any Body IR exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct BodyOwnerData {
    pub owner: SavedBodyOwner,
    pub module: ModuleRef,
    pub file: FileId,
    pub span: Span,
}

impl BodyOwnerData {
    /// Select outer owners in the same order as lowering. A declaration without a source body
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
        let functions = items
            .functions()
            .iter_with_ids()
            .filter_map(move |(id, data)| {
                if !data.signature.has_body() {
                    return None;
                }
                Some(Self {
                    owner: SavedBodyOwner::Function(id),
                    module: module_for(data.owner)?,
                    file: data.source.file_id,
                    span: data.span,
                })
            });
        let consts = items
            .consts()
            .iter_with_ids()
            .filter_map(move |(id, data)| {
                if !data.signature.has_value() {
                    return None;
                }
                Some(Self {
                    owner: SavedBodyOwner::Const(id),
                    module: module_for(data.owner)?,
                    file: data.source.file_id,
                    span: data.span,
                })
            });
        let statics = items
            .statics()
            .iter_with_ids()
            .filter_map(move |(id, data)| {
                // Statics lack the function signature's `has_body` bit. An extern static's
                // declaration sits in a foreign block and has no initializer to lower.
                definitions
                    .foreign_block(data.local_def.local_def)
                    .is_none()
                    .then_some(Self {
                        owner: SavedBodyOwner::Static(id),
                        module: data.owner,
                        file: data.source.file_id,
                        span: data.span,
                    })
            });
        functions.chain(consts).chain(statics)
    }
}
