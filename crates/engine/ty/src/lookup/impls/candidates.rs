//! Narrow the semantic declaration index using a receiver's outer type shape.

use rg_ir_model::{TraitDefRef, TraitImplRef};
use rg_semantic_ir::TraitImplSelfHead;
use rg_std::UniqueVec;

use crate::{
    Ty, TyContext,
    solver::{self, TyShape},
};

/// The receiver evidence available to source-impl discovery. This only narrows declarations;
/// the solver still relates their generic arguments and proves their predicates.
#[derive(Debug, Clone, Copy)]
pub(crate) enum TraitImplFilter {
    /// A parameter, projection, or live variable does not identify a concrete receiver family.
    All,
    /// Include this concrete family and the blanket/unresolved-header fallbacks.
    Head(TraitImplSelfHead),
    /// A closure or function item cannot be named by a concrete source impl.
    Fallbacks,
    /// An unknown owned type supplies no evidence; do not guess its type from visible impls.
    NoEvidence,
}

impl TraitImplFilter {
    pub(crate) fn candidates<D, I>(
        self,
        context: &TyContext<'_, D, I>,
        trait_ref: TraitDefRef,
    ) -> Option<UniqueVec<TraitImplRef>> {
        if context.cancellation().is_cancelled() {
            return None;
        }
        let lookup = context.item_lookup();
        let candidates = match self {
            Self::All => lookup.trait_impls_for_trait(trait_ref),
            Self::Head(head) => lookup.trait_impl_candidates_for_self_head(trait_ref, Some(head)),
            Self::Fallbacks => lookup.trait_impl_candidates_for_self_head(trait_ref, None),
            Self::NoEvidence => return Some(UniqueVec::new()),
        };
        Some(candidates.ok()?.unwrap_or_default())
    }
}

impl From<&Ty> for TraitImplFilter {
    fn from(receiver: &Ty) -> Self {
        let head = match receiver {
            Ty::Unknown => return Self::NoEvidence,
            Ty::Param(_) | Ty::Alias(_) => return Self::All,
            Ty::Unit => TraitImplSelfHead::Unit,
            Ty::Never => TraitImplSelfHead::Never,
            Ty::Primitive(primitive) => TraitImplSelfHead::Primitive(*primitive),
            Ty::Tuple(fields) => match u32::try_from(fields.len()) {
                Ok(arity) => TraitImplSelfHead::Tuple(arity),
                Err(_) => return Self::Fallbacks,
            },
            Ty::Array { .. } => TraitImplSelfHead::Array,
            Ty::Slice(_) => TraitImplSelfHead::Slice,
            Ty::Reference { mutability, .. } => TraitImplSelfHead::Reference(*mutability),
            Ty::RawPointer { mutability, .. } => TraitImplSelfHead::RawPointer(*mutability),
            Ty::FnPointer { params, .. } => match u32::try_from(params.len()) {
                Ok(arity) => TraitImplSelfHead::FnPointer(arity),
                Err(_) => return Self::Fallbacks,
            },
            Ty::Adt(ty) => TraitImplSelfHead::Adt(ty.def),
            Ty::Closure(_) | Ty::FnDef(_) => return Self::Fallbacks,
        };
        Self::Head(head)
    }
}

impl From<solver::Ty<'_>> for TraitImplFilter {
    /// Impl discovery only needs the outer constructor of `Vec<?T>`, not an owned copy of ?T
    /// and every nested argument. Unresolved solver types keep the conservative all-impl search;
    /// an owned `Ty::Unknown` instead has no inference state to support that search.
    fn from(receiver: solver::Ty<'_>) -> Self {
        let head = match receiver.shape() {
            TyShape::Unit => TraitImplSelfHead::Unit,
            TyShape::Never => TraitImplSelfHead::Never,
            TyShape::Primitive(primitive) => TraitImplSelfHead::Primitive(primitive),
            TyShape::Tuple(fields) => match u32::try_from(fields.len()) {
                Ok(arity) => TraitImplSelfHead::Tuple(arity),
                Err(_) => return Self::Fallbacks,
            },
            TyShape::Array { .. } => TraitImplSelfHead::Array,
            TyShape::Slice(_) => TraitImplSelfHead::Slice,
            TyShape::Reference { mutability, .. } => TraitImplSelfHead::Reference(mutability),
            TyShape::RawPointer { mutability, .. } => TraitImplSelfHead::RawPointer(mutability),
            TyShape::FnPointer { params, .. } => match u32::try_from(params.len()) {
                Ok(arity) => TraitImplSelfHead::FnPointer(arity),
                Err(_) => return Self::Fallbacks,
            },
            TyShape::Adt(adt) => TraitImplSelfHead::Adt(adt.def),
            TyShape::Closure(_) | TyShape::FnDef(_) => return Self::Fallbacks,
            TyShape::Param(_) | TyShape::Alias(_) | TyShape::InferVar { .. } | TyShape::Unknown => {
                return Self::All;
            }
        };
        Self::Head(head)
    }
}
