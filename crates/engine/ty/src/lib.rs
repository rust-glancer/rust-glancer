//! Type vocabulary shared by indexing and analysis layers.

mod associated_type;
mod autoderef;
mod call_arg;
mod deref;
mod generic_arg;
mod impl_match;
mod implementation;
mod item_path;
mod iteration;
mod member;
mod primitive;
mod ty;

pub use self::{
    autoderef::{
        Autoderef, AutoderefCandidate, AutoderefCandidates, AutoderefMode,
        ReferencePeelingCandidates,
    },
    call_arg::{CallArgInference, CallArgMapping, function_generic_shadow_subst},
    generic_arg::GenericArg,
    impl_match::ImplMatcher,
    implementation::ImplementationQuery,
    item_path::ItemPathQuery,
    iteration::IterationItemResolver,
    member::{MemberMethodCandidateRef, MemberMethodOrigin, MemberQuery},
    primitive::{FloatTy, PrimitiveTy, RefMutability, SignedIntTy, UnsignedIntTy},
    ty::{NominalTy, OpaqueTraitBound, Ty, TypeSubst},
};
