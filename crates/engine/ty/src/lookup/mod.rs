//! Find declarations by path or receiver type and retain the evidence from matching impls.

mod associated_item;
mod impl_match;
mod implementation;
mod item_path;
mod member;

pub use self::{
    associated_item::{AssociatedItemCandidateRef, AssociatedItemQuery, AssociatedItemRef},
    impl_match::{ImplMatcher, InherentImplMatch, ReceiverFunctionCandidate, ReceiverImplMatches},
    implementation::ImplementationQuery,
    item_path::ItemPathQuery,
    member::{MemberMethodCandidateRef, MemberMethodOrigin, MemberQuery},
};
