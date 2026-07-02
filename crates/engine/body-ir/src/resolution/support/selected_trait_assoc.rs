//! Projection for associated types written on selected trait methods.
//!
//! Selected trait methods have one extra fact that plain type-ref resolution does not carry:
//! which receiver type selected the trait method. That matters for syntax such as
//! `Self::Output` in a trait method signature, because the associated alias has to be read from
//! the receiver impl for that selected `Self` type.
//!
//! In simple words: imagine that `value.produce()` resolves to a trait method whose return type is
//! `Self::Output`. You know the selected receiver type at the call site, and you know which trait
//! supplied the method. This helper turns those two facts into the trait goal needed to project the
//! associated type.
//!
//! To resolve that, we build a selected-method trait goal and delegate actual impl projection to
//! the shared impl-predicate projector.

use rg_ir_model::{FunctionRef, ItemOwner, TraitRef, items::TypeRef};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_ty::{
    TraitGoal, TraitSelectionOptions, TraitSelectionQuery, Ty,
    inference::{InferTy, InferenceTable},
};

use crate::resolution::BodyResolutionContext;

use super::{ImplPredicateAssocProjection, ImplPredicateAssocProjector};

/// Selected trait-method context needed to interpret `Self::Assoc` syntax.
///
/// Basically, "we think the method comes from this trait, and we have this
/// receiver type at call site", e.g. for `value.produce()`
/// - `trait_ref` would correspond to `Produces`
/// - `selected_self_ty` would correspond to `Adapter<User>`
///
/// Note that at this stage we have not done impl matching yet, so Self ty
/// is not necessarily `Foo` from `impl Produces for Foo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SelectedTraitMethodContext<'a> {
    trait_ref: TraitRef,
    selected_self_ty: &'a Ty,
}

impl<'a> SelectedTraitMethodContext<'a> {
    pub(crate) fn trait_ref(&self) -> TraitRef {
        self.trait_ref
    }

    pub(crate) fn selected_self_ty(&self) -> &'a Ty {
        self.selected_self_ty
    }

    /// Build selected-trait context from an already selected associated function.
    ///
    /// Inherent calls and free functions have no trait `Self`, so they cannot project `Self::Assoc`
    /// through this helper. Trait-level generics are intentionally left unsupported until method
    /// selection carries their concrete arguments.
    pub(crate) fn from_function<'query, D, I>(
        context: BodyResolutionContext<'query, D, I>,
        function: FunctionRef,
        owner: ItemOwner,
        selected_self_ty: Option<&'a Ty>,
    ) -> Result<Option<Self>, PackageStoreError>
    where
        D: DefMapSource<Error = PackageStoreError> + Copy,
        I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
    {
        let ItemOwner::Trait(trait_id) = owner else {
            return Ok(None);
        };
        let Some(selected_self_ty) = selected_self_ty else {
            return Ok(None);
        };

        let trait_ref = TraitRef {
            origin: function.origin,
            id: trait_id,
        };
        let Some(trait_data) = context.item_query().trait_data(trait_ref)? else {
            return Ok(None);
        };
        if !trait_data.generics.lifetimes.is_empty()
            || !trait_data.generics.types.is_empty()
            || !trait_data.generics.consts.is_empty()
        {
            // TODO: Thread trait-level generic args from method selection before projecting
            // `Self::Assoc` for traits shaped like `Trait<T>`.
            return Ok(None);
        }

        Ok(Some(Self {
            trait_ref,
            selected_self_ty,
        }))
    }
}

/// Projects `Self::Assoc` through the unique impl selected by a trait method receiver.
pub(crate) struct SelectedTraitAssocProjector<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> SelectedTraitAssocProjector<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Project an associated type into inference form using the caller's table.
    ///
    /// Callers that are allowed to commit receiver evidence can adopt the returned table. Callers
    /// that only need a concrete fallback can use `project_concrete_ty` instead.
    pub(crate) fn project_infer_ty(
        &self,
        selected_method: &SelectedTraitMethodContext<'_>,
        assoc_name: &str,
        table: &InferenceTable,
    ) -> Result<Option<ImplPredicateAssocProjection>, PackageStoreError> {
        let goal = self.selected_goal(selected_method);
        let Some(projection) = TraitSelectionQuery::with_index(
            self.context.item_paths(),
            self.context.target_items(),
            self.context.semantic_index(),
        )
        // This projector is a body-local bridge from an already selected trait method to
        // `Self::Assoc`. The body obligation pass handles explicit where-clause evidence, so this
        // step only needs the impl header and must keep rejecting generic-parameter bounds.
        .with_options(TraitSelectionOptions::new().caller_solves_where_predicates())
        .normalize_assoc_type(&goal, assoc_name, table)?
        else {
            return Ok(None);
        };

        Ok(Some(ImplPredicateAssocProjection::new(
            projection.ty,
            projection.table,
        )))
    }

    /// Project an associated type into a stable concrete type for non-mutating callers.
    pub(crate) fn project_concrete_ty(
        &self,
        selected_method: &SelectedTraitMethodContext<'_>,
        assoc_name: &str,
    ) -> Result<Option<Ty>, PackageStoreError> {
        let table = InferenceTable::new();
        // Prefer the body-local impl-predicate path first. It can use simple support predicates
        // such as `S: Source` without mutating caller inference. If that cannot answer, fall
        // back to the shared associated-type normalizer for direct aliases.
        let projection = if let Some(projection) =
            self.project_concrete_infer_ty(selected_method, assoc_name, &table)?
        {
            Some(projection)
        } else {
            self.project_infer_ty(selected_method, assoc_name, &table)?
        };
        let Some(projection) = projection else {
            return Ok(None);
        };
        let (projected_ty, table) = projection.into_parts();
        let projected_ty = table.finalize(&projected_ty);
        if matches!(projected_ty, Ty::Syntax(_)) || projected_ty.has_unknown() {
            return Ok(Some(Ty::Unknown));
        }

        Ok(Some(projected_ty))
    }

    /// Project concrete selected aliases using the impl predicates that the project understands
    /// locally.
    ///
    /// This is the early, non-mutating companion to the final body-obligation path. It is enough
    /// for aliases such as `Adapter<S>::Output = S::Item`: the `S: Source` support predicate tells
    /// us how to normalize `S::Item`, while unrelated callable predicates are ignored because they
    /// do not contribute to that alias value. Aliases that require callable return solving, such as
    /// `CallAdapter<S, F>::Output = B` with `F: FnMut(S::Item) -> B`, stay unknown here and are
    /// handled by the final inference pass where closure bodies are available.
    fn project_concrete_infer_ty(
        &self,
        selected_method: &SelectedTraitMethodContext<'_>,
        assoc_name: &str,
        table: &InferenceTable,
    ) -> Result<Option<ImplPredicateAssocProjection>, PackageStoreError> {
        let goal = self.selected_goal(selected_method);
        ImplPredicateAssocProjector::new(self.context)
            .project_goal_through_impl_predicates(&goal, assoc_name, table)
    }

    /// Build the trait goal represented by this selected method call.
    ///
    /// Trait-level generic args are not threaded through selected method context yet, so selected
    /// contexts are only built for traits without such params.
    fn selected_goal(&self, selected_method: &SelectedTraitMethodContext<'_>) -> TraitGoal {
        TraitGoal {
            self_ty: InferTy::from_ty(selected_method.selected_self_ty),
            trait_ref: selected_method.trait_ref,
            args: Vec::new(),
        }
    }
}

pub(crate) fn self_associated_type_name(ty: &TypeRef) -> Option<&str> {
    // TODO: Generalize this replacement to nested shapes such as `Option<Self::Item>` when
    // selected-call obligations need them.
    let TypeRef::Path(path) = ty else {
        return None;
    };
    let [self_segment, assoc_segment] = path.segments.as_slice() else {
        return None;
    };
    if path.absolute
        || self_segment.name.as_str() != "Self"
        || !self_segment.args.is_empty()
        || !assoc_segment.args.is_empty()
    {
        return None;
    }

    Some(assoc_segment.name.as_str())
}
