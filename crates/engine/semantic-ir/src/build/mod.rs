//! Lower and publish semantic declarations for one target.

mod body_owners;
pub(crate) mod impl_headers;
mod item_store;
mod lower_crate;
mod target;

pub use self::{
    body_owners::BodyOwnerData,
    item_store::{ItemStoreLowerer, ItemStoreSourceReader},
    target::{
        TargetDeclarationMetadata, TargetDeclarationReader, TargetDeclarations, target_declarations,
    },
};
