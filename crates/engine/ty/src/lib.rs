//! Canonical semantic types shared by indexing and body analysis.
//!
//! `rg_semantic_ir` owns source-shaped declaration signatures. This crate crosses that syntax
//! boundary once, producing owner-scoped parameters, full generic argument lists, and semantic
//! clauses. Inference, impl matching, associated-type projection, and Chalk all consume those same
//! shapes instead of maintaining their own `TypeRef` lowering rules.

mod autoderef;
mod context;
mod deref;
mod generic_arg;
mod impl_match;
mod implementation;
pub mod inference;
mod item_path;
mod lowering;
mod member;
mod primitive_expr;
mod profile;
mod signature;
mod substitution;
mod trait_selection;
mod ty;

pub use rg_ir_model::{FloatTy, Mutability, PrimitiveTy, SignedIntTy, UnsignedIntTy};

pub use self::{
    autoderef::{
        Autoderef, AutoderefCandidate, AutoderefCandidates, AutoderefMode,
        ReferencePeelingCandidates,
    },
    context::TyContext,
    generic_arg::{
        AssocTypeBinding, Clause, ConstValue, GenericArg, GenericArgs, Lifetime, TraitApplication,
        TraitRefLowering,
    },
    impl_match::ImplMatcher,
    implementation::ImplementationQuery,
    item_path::ItemPathQuery,
    lowering::{
        TypeLoweringAnchor, TypeLoweringEnv, TypeLoweringQuery, TypeLoweringSession,
        TypePathResolver,
    },
    member::{MemberMethodCandidateRef, MemberMethodOrigin, MemberQuery},
    primitive_expr::{ty_for_binary, ty_for_literal, ty_for_unary},
    profile::profile_descriptors,
    signature::{CallableSignature, ImplHeader, SemanticSignatureQuery},
    substitution::Substitution,
    trait_selection::{
        AssocProjectionResult, TraitGoal, TraitProof, TraitSelection, TraitSelectionQuery,
        TraitSelectionSession,
    },
    ty::{
        AdtTy, AliasTy, ClosureTy, ClosureTyId, ExpectedAdtTyExt, ExpectedTyExt, FnDefTy, OpaqueTy,
        ProjectionTy, Ty,
    },
};
