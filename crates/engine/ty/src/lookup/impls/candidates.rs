//! Narrow the semantic declaration index using a receiver's outer type shape.

use rg_ir_model::{TraitDefRef, TraitImplRef};
use rg_semantic_ir::{ItemLookupQuery, TraitImplSelfHead};
use rg_source::ReadError;
use rg_std::UniqueVec;

use crate::{
    PrimitiveTy, Ty, TyContext,
    solver::{self, InferVarKind, TyShape},
};

/// The receiver evidence available to source-impl discovery. This only narrows declarations;
/// the solver still relates their generic arguments and proves their predicates.
#[derive(Debug, Clone, Copy)]
pub(crate) enum TraitImplFilter {
    /// A parameter, projection, or unconstrained type variable can have any receiver type.
    All,
    /// Include this concrete family and the blanket/unresolved-header fallbacks.
    Head(TraitImplSelfHead),
    /// An unsuffixed integer can still become any signed or unsigned integer type.
    Integers,
    /// An unsuffixed float can still become any floating-point type.
    Floats,
    /// The index has no separate keys for closures, function items, or trait objects.
    /// Search its fallback list, which includes blanket impls such as `impl<T> Trait for T`.
    Fallbacks,
    /// An unknown owned type supplies no evidence; do not guess its type from visible impls.
    NoEvidence,
}

impl TraitImplFilter {
    pub(crate) fn candidates<D, I>(
        self,
        context: &TyContext<'_, D, I>,
        trait_ref: TraitDefRef,
    ) -> Result<UniqueVec<TraitImplRef>, ReadError> {
        rg_std::check_cancel!(context, "trait impl candidates");
        let lookup = context.item_lookup();
        let candidates = match self {
            Self::All => lookup.trait_impls_for_trait(trait_ref),
            Self::Head(head) => lookup.trait_impl_candidates_for_self_head(trait_ref, Some(head)),
            Self::Integers => {
                return Self::primitive_candidates(lookup, trait_ref, PrimitiveTy::is_integral);
            }
            Self::Floats => {
                return Self::primitive_candidates(lookup, trait_ref, PrimitiveTy::is_float);
            }
            Self::Fallbacks => lookup.trait_impl_candidates_for_self_head(trait_ref, None),
            Self::NoEvidence => return Ok(UniqueVec::new()),
        };
        Ok(candidates?.unwrap_or_default())
    }

    fn primitive_candidates(
        lookup: &ItemLookupQuery<'_>,
        trait_ref: TraitDefRef,
        accepts: impl Fn(PrimitiveTy) -> bool,
    ) -> Result<UniqueVec<TraitImplRef>, ReadError> {
        // Each primitive lookup includes blanket and unresolved impls. An impl for a type
        // alias can come from that list, so keep shared candidates once in the union.
        let mut candidates = UniqueVec::new();
        for primitive in PrimitiveTy::ALL.iter().copied().filter(|&ty| accepts(ty)) {
            let head = TraitImplSelfHead::Primitive(primitive);
            if let Some(impls) =
                lookup.trait_impl_candidates_for_self_head(trait_ref, Some(head))?
            {
                candidates.extend(impls);
            }
        }
        Ok(candidates)
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
            Ty::Closure(_) | Ty::FnDef(_) | Ty::Dyn(_) => return Self::Fallbacks,
        };
        Self::Head(head)
    }
}

impl From<solver::Ty<'_>> for TraitImplFilter {
    /// Impl discovery only needs the outer constructor of `Vec<?T>`. Even before the type of
    /// `2` in `require_debug(2)` settles, its integer kind narrows the possible `Debug` impls.
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
            TyShape::Closure(_) | TyShape::FnDef(_) | TyShape::Dyn(_) => return Self::Fallbacks,
            TyShape::InferVar { kind } => {
                return match kind {
                    InferVarKind::Integer => Self::Integers,
                    InferVarKind::Float => Self::Floats,
                    InferVarKind::Type => Self::All,
                };
            }
            TyShape::Param(_) | TyShape::Alias(_) | TyShape::Unknown => {
                return Self::All;
            }
        };
        Self::Head(head)
    }
}
