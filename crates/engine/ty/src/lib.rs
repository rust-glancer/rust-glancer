//! Storage-agnostic type vocabulary shared by indexing and analysis layers.
//!
//! The crate owns the common shape of type facts, not the stores that answer questions about those
//! facts. Concrete IR layers provide the representation payload for resolved or preserved
//! storage-specific type facts.

mod generic_arg;
mod indexed;
mod primitive;
mod ty;

pub use self::{
    generic_arg::GenericArg,
    indexed::{
        IndexedGenericArg, IndexedLocalNominalTy, IndexedNominalTy, IndexedTy, IndexedTyExt,
        IndexedTyRepr, IndexedTypeSubst,
    },
    primitive::{FloatTy, PrimitiveTy, RefMutability, SignedIntTy, UnsignedIntTy},
    ty::{Ty, TypeRepr, TypeSubst},
};
