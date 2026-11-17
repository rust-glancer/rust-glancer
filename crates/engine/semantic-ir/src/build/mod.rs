//! Lower and publish semantic declarations for one target.

mod body_owners;
pub(crate) mod impl_headers;
mod item_store;
mod lang_items;
mod lookup;
mod lower_crate;
mod members;
mod symbol_names;
mod target;

pub use self::{
    body_owners::BodyOwnerData,
    item_store::{ItemStoreLowerer, ItemStoreSourceReader},
    members::{AssociatedDeclarations, associated_declarations, body_owner, body_owners},
    symbol_names::{SymbolNameData, SymbolNameIndex},
    target::{
        TargetDeclarationMetadata, TargetDeclarationReader, TargetDeclarations, TargetItemData,
        TargetSourceFiles, target_declarations,
    },
};
