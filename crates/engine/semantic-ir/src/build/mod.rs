//! Lower and publish semantic declarations for one target.

mod body_owners;
pub(crate) mod impl_headers;
mod item_store;
mod lookup;
mod lower_crate;
mod symbol_names;
mod target;

pub use self::{
    body_owners::BodyOwnerData,
    item_store::{ItemStoreLowerer, ItemStoreSourceReader},
    symbol_names::SymbolNameData,
    target::{
        TargetDeclarationMetadata, TargetDeclarationReader, TargetDeclarations, target_declarations,
    },
};
