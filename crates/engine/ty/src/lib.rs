//! Type vocabulary shared by indexing and analysis layers.

mod generic_arg;
mod primitive;
mod ty;

pub use self::{
    generic_arg::GenericArg,
    primitive::{FloatTy, PrimitiveTy, RefMutability, SignedIntTy, UnsignedIntTy},
    ty::{NominalTy, Ty, TypeSubst},
};
