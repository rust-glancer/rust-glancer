//! Type vocabulary shared by indexing and analysis layers.

mod autoderef;
mod deref;
mod generic_arg;
mod impl_match;
mod item_path;
mod primitive;
mod ty;

pub use self::{
    autoderef::{
        Autoderef, AutoderefCandidate, AutoderefCandidates, AutoderefMode,
        ReferencePeelingCandidates,
    },
    generic_arg::GenericArg,
    impl_match::ImplMatcher,
    item_path::ItemPathQuery,
    primitive::{FloatTy, PrimitiveTy, RefMutability, SignedIntTy, UnsignedIntTy},
    ty::{NominalTy, Ty, TypeSubst},
};
