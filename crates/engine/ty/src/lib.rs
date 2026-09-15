//! Canonical semantic types shared by indexing and body analysis.
//!
//! `rg_semantic_ir` owns source-shaped declaration signatures. This crate crosses that syntax
//! boundary once, producing owner-scoped parameters, full generic argument lists, and semantic
//! clauses. Inference, impl matching, associated-type projection, and Chalk all consume those same
//! shapes instead of maintaining their own `TypeRef` lowering rules.
//!
//! Shared type shapes and substitutions are available at the crate root. `lowering` interprets
//! source types and declaration signatures; `lookup` finds declarations by path or receiver type.
//! `autoderef` adjusts receivers, `inference` tracks temporary variables, and `trait_selection`
//! proves predicates and normalizes associated types.

pub mod autoderef;
mod context;
mod generic_arg;
pub mod inference;
pub mod lookup;
pub mod lowering;
mod primitive_expr;
mod profile;
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
