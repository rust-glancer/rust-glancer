//! Type vocabulary shared by indexing and analysis layers.

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
mod member;
mod primitive_expr;
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
    generic_arg::GenericArg,
    impl_match::ImplMatcher,
    implementation::ImplementationQuery,
    item_path::ItemPathQuery,
    iteration::IterationItemResolver,
    member::{MemberMethodCandidateRef, MemberMethodOrigin, MemberQuery},
    primitive_expr::{ty_for_binary, ty_for_literal, ty_for_unary},
    trait_selection::{TraitGoal, TraitSelection, TraitSelectionOptions, TraitSelectionQuery},
    ty::{
        ClosureTyId, ExpectedNominalTyExt, ExpectedTyExt, NominalTy, OpaqueTraitBound, Ty,
        TypeSubst,
    },
};
