//! The compiler trait solver and the temporary types used while inferring a body.
//!
//! A solver operation borrows its declarations and owns all interned values until its results
//! have been converted to durable semantic types. Nothing in this module is stored in BodyFacts.
//! The solver's inference variables are also the variables used by body inference.
//!
//! `infer` implements the compiler's inference interface. `inference` adds the caller's assumptions
//! and pending obligations; `query` matches declarations and normalizes projections in that live
//! table. `autoderef` walks references and trait `Deref` targets using that same table.
//! `declarations` supplies source metadata and the cache shared within one lexical context.
//!
//! The compiler reads that data through `interner` callbacks. A `CallbackScope` records when
//! those reads cannot supply the requested information, so queries can reject an answer based
//! on fallback data even if the compiler solver itself reports success.

mod autoderef;
mod conversion;
mod declarations;
mod delegate;
mod fold;
mod infer;
mod inference;
mod interner;
mod profile;
mod query;
mod shape;
mod traverse;
mod types;

pub(crate) use self::declarations::{DeclarationKind, DeclarationMetadata, DeclarationProvider};
pub use self::{
    autoderef::Autoderef,
    declarations::{DeclarationCache, SemanticDeclarations, SolverScope},
    delegate::{Outcome, Solver},
    infer::InferCtxt,
    inference::{InferenceConflict, InferenceSubstitution, InferenceTable},
    interner::{CallbackScope, SolverInterner, SolverStorage},
    query::{
        AssocTypeBinding, CallableSignature, ImplHeader, ImplSelection, TraitApplication,
        TraitRefLowering,
    },
    shape::{AdtTy, AliasTy, ClosureTy, FnDefTy, InferVarKind, OpaqueTy, ProjectionTy, TyShape},
    types::{
        Clause, Const, DefId, GenericArg, GenericArgs, List, Param, ParamEnv, Predicate, Region,
        Term, Ty,
    },
};
