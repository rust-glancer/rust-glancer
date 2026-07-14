//! Canonical semantic types shared by indexing and body analysis.
//!
//! `rg_semantic_ir` owns source-shaped declaration signatures. This crate crosses that syntax
//! boundary once, producing owner-scoped parameters, full generic argument lists, and semantic
//! clauses. Inference, impl matching, associated-type projection, and Chalk all consume those same
//! shapes instead of maintaining their own `TypeRef` lowering rules.

mod associated_type;
mod autoderef;
mod call_arg;
mod deref;
mod generic_arg;
mod impl_match;
mod implementation;
pub mod inference;
mod item_path;
mod iteration;
mod lowering;
mod member;
mod primitive_expr;
mod profile;
mod signature;
mod substitution;
mod trait_selection;
mod ty;

pub use rg_ir_model::{
    Mutability,
    items::{FloatTy, PrimitiveTy, SignedIntTy, UnsignedIntTy},
};

pub use self::{
    autoderef::{
        Autoderef, AutoderefCandidate, AutoderefCandidates, AutoderefMode,
        ReferencePeelingCandidates,
    },
    call_arg::{CallArgInference, CallArgMapping, function_generic_shadow_subst},
    generic_arg::{
        AssocTypeBinding, Clause, ConstValue, GenericArg, GenericArgs, Lifetime, TraitApplication,
        TraitRefLowering,
    },
    impl_match::ImplMatcher,
    implementation::ImplementationQuery,
    item_path::ItemPathQuery,
    iteration::IterationItemResolver,
    lowering::{
        TypeLoweringAnchor, TypeLoweringEnv, TypeLoweringQuery, TypeLoweringSession,
        TypePathResolver,
    },
    member::{MemberMethodCandidateRef, MemberMethodOrigin, MemberQuery},
    primitive_expr::{ty_for_binary, ty_for_literal, ty_for_unary},
    profile::profile_descriptors,
    signature::{CallableSignature, ImplHeader, SemanticSignatureQuery, TraitHeader},
    substitution::Substitution,
    trait_selection::{
        AssocProjectionResult, TraitGoal, TraitSelection, TraitSelectionCache,
        TraitSelectionOptions, TraitSelectionQuery,
    },
    ty::{
        AdtTy, AliasTy, ClosureTyId, ExpectedAdtTyExt, ExpectedTyExt, FnDefTy, OpaqueTy,
        ProjectionTy, Ty,
    },
};
