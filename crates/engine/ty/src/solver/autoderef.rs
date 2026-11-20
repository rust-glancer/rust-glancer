//! Autoderef shared by inference and editor queries.
//!
//! Walk from a type through references and trait `Deref` targets, using the caller's inference
//! table and assumptions. For example, `T: Deref<Target = Widget>` supplies a step from `T` to
//! `Widget` without a concrete impl. Method lookup adds an array-to-slice alternative separately.

use rg_ir_model::Mutability;
use rg_std::UniqueVec;

use super::{
    DefId, InferenceTable, List, Outcome, ProjectionTy, TraitApplication, Ty, TyShape,
    declarations::LangItem,
};

// Bound malformed chains even when every step creates a different type. Repeated types stop
// earlier. The original type counts toward this limit.
const MAX_AUTODEREF_TYPES: usize = 32;

/// A lazy dereference chain borrowing the caller's live inference state.
/// Yield the original type before computing a step: a direct field or method needs no `Deref`
/// goal. Later steps can retain inference variables whose values the caller learns afterward.
/// A mutable walk remembers the source of each trait-backed step so coercion can prove
/// `DerefMut` after matching the expected pointee.
pub struct Autoderef<'table, 's> {
    table: &'table InferenceTable<'s>,
    current: Option<Ty<'s>>,
    seen: UniqueVec<Ty<'s>>,
    // A reference coercion supplies an expected pointee and the access it needs.
    coercion_mutability: Option<Mutability>,
    mutable_deref_sources: Vec<Ty<'s>>,
}

impl<'table, 's> Autoderef<'table, 's> {
    fn new(
        table: &'table InferenceTable<'s>,
        ty: Ty<'s>,
        coercion_mutability: Option<Mutability>,
    ) -> Self {
        Self {
            table,
            current: Some(ty),
            seen: UniqueVec::new(),
            coercion_mutability,
            mutable_deref_sources: Vec::new(),
        }
    }
}

impl<'s> Iterator for Autoderef<'_, 's> {
    type Item = Ty<'s>;

    fn next(&mut self) -> Option<Self::Item> {
        let table = self.table;
        let cx = table.interner();
        let requires_mutable_access = self.coercion_mutability == Some(Mutability::Mutable);
        let ty = table.resolve_root_var(self.current.take()?);
        let ty = if self.seen.is_empty() {
            ty
        } else {
            match ty.shape() {
                _ if self.seen.len() >= MAX_AUTODEREF_TYPES => return None,
                TyShape::Reference {
                    inner, mutability, ..
                } if !requires_mutable_access || mutability == Mutability::Mutable => inner,
                TyShape::Adt(_) | TyShape::Param(_) | TyShape::Alias(_) | TyShape::Dyn(_) => {
                    let callbacks = cx.track_callbacks();
                    let DefId::Trait(deref) = cx.lang_item(LangItem::Deref)? else {
                        return None;
                    };
                    let DefId::TypeAlias(alias) = cx.lang_item(LangItem::DerefTarget)? else {
                        return None;
                    };
                    // These language items are indexed independently. An unrelated alias
                    // with the target attribute must not supply this trait's projection.
                    if cx.metadata(DefId::TypeAlias(alias)).parent != Some(DefId::Trait(deref)) {
                        return None;
                    }
                    let target = table.normalize(cx.projection(ProjectionTy {
                        associated_ty: alias,
                        args: List::new(cx, &[ty.into()]),
                    }));
                    let outcome = table.fulfill();
                    let target = table.resolve_root_var(target);
                    // A proved projection can still yield a variable, as with Box<?T>.
                    // Reference coercion can constrain it by matching the expected pointee.
                    let can_match_variable =
                        self.coercion_mutability.is_some() && outcome == Outcome::Proven;
                    if callbacks.failure().is_some()
                        || target.has_unknown()
                        || (target.is_var() && !can_match_variable)
                    {
                        return None;
                    }
                    if requires_mutable_access {
                        self.mutable_deref_sources.push(ty);
                    }
                    target
                }
                _ => return None,
            }
        };
        let ty = table.resolve_root_var(ty);
        if !self.seen.push(ty) {
            return None;
        }
        self.current = Some(ty);
        Some(ty)
    }
}

impl<'s> InferenceTable<'s> {
    pub fn autoderef(&self, ty: Ty<'s>) -> Autoderef<'_, 's> {
        Autoderef::new(self, ty, None)
    }

    /// Relate reference pointees after an exact match has failed. For example, `&Vec<Item>`
    /// can satisfy `&[?T]` through Deref, which also learns `?T = Item`.
    pub fn try_coerce_reference(&self, source: Ty<'s>, target: Ty<'s>) -> bool {
        let (Some((source, source_mutability)), Some((target, target_mutability))) = (
            self.resolve_root_var(source).reference_inner(),
            self.resolve_root_var(target).reference_inner(),
        ) else {
            return false;
        };
        if source_mutability == Mutability::Shared && target_mutability == Mutability::Mutable {
            return false;
        }

        let source = self.resolve_root_var(source);
        let target = self.resolve_root_var(target);
        if let (TyShape::Array { inner, .. }, TyShape::Slice(element)) =
            (source.shape(), target.shape())
        {
            // Unsizing the original array reference relates the elements without using its length.
            return self.try_unify(inner, element).is_ok();
        }
        if source_mutability == Mutability::Mutable
            && target_mutability == Mutability::Shared
            && self.try_unify(source, target).is_ok()
        {
            return true;
        }

        // Deref can register projections and refine variables. Keep that work only for a
        // matching pointee that can be reached with the required access.
        self.commit_if_some(|table| -> Result<_, std::convert::Infallible> {
            let mut deref = Autoderef::new(table, source, Some(target_mutability));
            let matched = deref
                .by_ref()
                .skip(1)
                .any(|pointee| table.try_unify(pointee, target).is_ok());
            if !matched {
                return Ok(None);
            }
            if deref.mutable_deref_sources.is_empty() {
                return Ok(Some(()));
            }

            // Matching `[?T]` with `[u16]` can prove a bound on `Wrapper<?T>: DerefMut`.
            // Check every trait-backed step now that the match has supplied that evidence.
            let cx = table.interner();
            let Some(DefId::Trait(def)) = cx.lang_item(LangItem::DerefMut) else {
                return Ok(None);
            };
            let obligations = deref.mutable_deref_sources.into_iter().map(|source| {
                TraitApplication {
                    def,
                    args: List::new(cx, &[source.into()]),
                }
                .clause(cx)
            });
            Ok((table.prove(obligations) == Outcome::Proven).then_some(()))
        })
        .expect("reference coercion is infallible")
        .is_some()
    }

    /// Try each autoderef type as a method receiver. If the chain ends at `[T; N]`, also try
    /// `[T]`; this unsizing step is available even when the array reached the autoderef limit.
    pub fn method_receivers(&self, ty: Ty<'s>) -> impl Iterator<Item = Ty<'s>> {
        let mut autoderef = self.autoderef(ty);
        let mut slice_element = None;
        std::iter::from_fn(move || {
            // Delay even constructing the slice until lookup has tried the array itself.
            if let Some(inner) = slice_element.take() {
                return Some(self.interner().slice(inner));
            }
            let ty = autoderef.next()?;
            if let TyShape::Array { inner, .. } = ty.shape() {
                slice_element = Some(inner);
            }
            Some(ty)
        })
    }

    /// For each possible Self, Rust tries the value, a shared borrow, and a mutable borrow.
    pub fn method_receiver_adjustments(
        &self,
        self_types: &[Ty<'s>],
    ) -> impl Iterator<Item = Ty<'s>> {
        let cx = self.interner();
        self_types.iter().flat_map(move |&ty| {
            [
                ty,
                cx.reference(Mutability::Shared, ty),
                cx.reference(Mutability::Mutable, ty),
            ]
        })
    }
}
