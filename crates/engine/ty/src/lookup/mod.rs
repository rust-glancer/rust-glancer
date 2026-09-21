//! Find declarations by path or receiver type and retain the evidence from matching impls.

mod associated_item;
mod impl_match;
mod implementation;
mod item_path;
mod member;

pub use self::associated_item::{
    AssociatedItemCandidateRef, AssociatedItemQuery, AssociatedItemRef,
};
pub use self::impl_match::{
    ImplMatcher, InherentImplMatch, ReceiverFunctionCandidate, ReceiverImplMatches,
};
pub use self::implementation::ImplementationQuery;
pub use self::item_path::ItemPathQuery;
pub use self::member::{MemberMethodCandidateRef, MemberMethodOrigin, MemberQuery};
