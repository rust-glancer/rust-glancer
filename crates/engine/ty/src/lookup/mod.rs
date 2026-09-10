//! Find declarations by path or receiver type and retain the evidence from matching impls.

mod associated_item;
mod impl_match;
mod implementation;
mod item_path;
mod member;

pub use associated_item::{AssociatedItemCandidateRef, AssociatedItemQuery, AssociatedItemRef};
pub use impl_match::{
    ImplMatcher, InherentImplMatch, ReceiverFunctionCandidate, ReceiverImplMatches,
};
pub use implementation::ImplementationQuery;
pub use item_path::ItemPathQuery;
pub use member::{MemberMethodCandidateRef, MemberMethodOrigin, MemberQuery};
