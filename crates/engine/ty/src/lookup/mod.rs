//! Find declarations by path or receiver type and retain the evidence from matching impls.

mod associated_item;
mod implementation;
mod impls;
mod item_path;
mod member;

pub(crate) use self::impls::TraitImplFilter;
pub use self::{
    associated_item::{AssociatedItemCandidateRef, AssociatedItemQuery, AssociatedItemRef},
    implementation::ImplementationQuery,
    impls::{ImplQuery, InherentImplMatch, ReceiverFunctionCandidate, ReceiverImplMatches},
    item_path::ItemPathQuery,
    member::{MemberMethodCandidateRef, MemberMethodOrigin},
};
