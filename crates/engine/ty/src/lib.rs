//! Source-type interpretation, inference, and independent type results.
//!
//! `rg_semantic_ir` owns source-shaped declaration signatures. This crate crosses that syntax
//! boundary through one lowerer, producing compiler-compatible types in an operation's storage.
//! Inference, impl matching, and associated-type projection keep those working types, including
//! their live variables. Saved facts and independent queries export owned results before the
//! operation releases its storage.
//!
//! Owned type shapes and substitutions are available at the crate root. `lowering` interprets
//! source types and declaration signatures; `signature` queries declarations for owned types and
//! bounds. `lookup` finds declarations by path or receiver type, `autoderef` adjusts receivers,
//! `solver` owns temporary inference and trait proof, and `trait_selection` freezes results for
//! editor queries.

pub mod autoderef;
mod context;
mod generic_arg;
pub mod lookup;
pub mod lowering;
mod primitive_expr;
mod profile;
pub mod signature;
pub mod solver;
mod substitution;
pub mod trait_selection;
mod ty;

pub use rg_ir_model::{FloatTy, Mutability, PrimitiveTy, SignedIntTy, UnsignedIntTy};

pub use self::{
    context::TyContext,
    generic_arg::{
        AssocTypeBinding, Clause, ConstValue, GenericArg, GenericArgs, Lifetime, TraitApplication,
        TraitRefLowering,
    },
    primitive_expr::{ty_for_binary, ty_for_literal, ty_for_unary},
    profile::profile_descriptors,
    substitution::Substitution,
    ty::{
        AdtTy, AliasTy, ClosureTy, ClosureTyId, ExpectedAdtTyExt, ExpectedTyExt, FnDefTy, OpaqueTy,
        ProjectionTy, Ty,
    },
};
