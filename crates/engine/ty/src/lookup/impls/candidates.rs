//! Narrow the semantic declaration index using a receiver's outer type shape.

use rg_ir_model::{TraitDefRef, TraitImplRef};
use rg_semantic_ir::TraitImplSelfHead;
use rg_std::UniqueVec;

use crate::{Ty, TyContext};

/// Shared discovery for owned queries and solver callbacks. This only chooses source impls;
/// the solver still relates their generic arguments and proves their predicates.
pub(crate) fn trait_impl_candidates<D, I>(
    context: &TyContext<'_, D, I>,
    trait_ref: TraitDefRef,
    receiver: &Ty,
) -> Option<UniqueVec<TraitImplRef>> {
    if context.cancellation().is_cancelled() {
        return None;
    }
    let lookup = context.item_lookup();
    let head = match receiver {
        // An unknown receiver must not acquire a type by guessing a source impl. A parameter or
        // projection can still select an impl, but provides no outer shape to narrow the index.
        Ty::Unknown => return Some(UniqueVec::new()),
        Ty::Param(_) | Ty::Alias(_) => {
            return Some(
                lookup
                    .trait_impls_for_trait(trait_ref)
                    .ok()?
                    .unwrap_or_default(),
            );
        }
        Ty::Unit => Some(TraitImplSelfHead::Unit),
        Ty::Never => Some(TraitImplSelfHead::Never),
        Ty::Primitive(primitive) => Some(TraitImplSelfHead::Primitive(*primitive)),
        Ty::Tuple(fields) => u32::try_from(fields.len())
            .ok()
            .map(TraitImplSelfHead::Tuple),
        Ty::Array { .. } => Some(TraitImplSelfHead::Array),
        Ty::Slice(_) => Some(TraitImplSelfHead::Slice),
        Ty::Reference { mutability, .. } => Some(TraitImplSelfHead::Reference(*mutability)),
        Ty::RawPointer { mutability, .. } => Some(TraitImplSelfHead::RawPointer(*mutability)),
        Ty::FnPointer { params, .. } => u32::try_from(params.len())
            .ok()
            .map(TraitImplSelfHead::FnPointer),
        Ty::Adt(ty) => Some(TraitImplSelfHead::Adt(ty.def)),
        // No source impl can name a particular closure or function item. `None` asks the index
        // for blanket and unresolved-header fallbacks, without unrelated concrete impls.
        Ty::Closure(_) | Ty::FnDef(_) => None,
    };
    Some(
        lookup
            .trait_impl_candidates_for_self_head(trait_ref, head)
            .ok()?
            .unwrap_or_default(),
    )
}
