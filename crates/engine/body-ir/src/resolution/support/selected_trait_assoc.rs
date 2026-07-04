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
//! To resolve that, we build a selected-method trait goal and delegate actual projection policy to
//! the body associated type projector.

use rg_ir_model::{FunctionRef, ItemOwner, TraitRef, items::TypeRef};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_ty::{TraitGoal, TraitSelectionCache, Ty, inference::InferenceTable};

use crate::resolution::BodyResolutionContext;

use super::BodyAssocProjector;

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
    trait_selection_cache: TraitSelectionCache,
}

impl<'query, D, I> SelectedTraitAssocProjector<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self {
            context,
            trait_selection_cache: TraitSelectionCache::default(),
        }
    }

    pub(crate) fn with_cache(mut self, cache: TraitSelectionCache) -> Self {
        self.trait_selection_cache = cache;
        self
    }

    /// Project an associated type into a stable concrete type for non-mutating callers.
    pub(crate) fn project_concrete_ty(
        &self,
        selected_method: &SelectedTraitMethodContext<'_>,
        assoc_name: &str,
    ) -> Result<Option<Ty>, PackageStoreError> {
        let table = InferenceTable::new();
        // Concrete callers still use the same body projection policy as mutating callers; they
        // simply finalize the trial table instead of committing it.
        let goal = self.selected_goal(selected_method);
        let projection = BodyAssocProjector::new(self.context)
            .with_cache(self.trait_selection_cache.clone())
            .normalize_assoc_type(&goal, assoc_name, &table)?;
        let Some(projection) = projection else {
            return Ok(None);
        };
        let (projected_ty, _applicability, table) = projection.into_parts();
        let projected_ty = table.finalize(&projected_ty);
        if matches!(projected_ty, Ty::Syntax(_)) || projected_ty.has_unknown() {
            return Ok(Some(Ty::Unknown));
        }

        Ok(Some(projected_ty))
    }

    /// Build the trait goal represented by this selected method call.
    ///
    /// Trait-level generic args are not threaded through selected method context yet, so selected
    /// contexts are only built for traits without such params.
    fn selected_goal(&self, selected_method: &SelectedTraitMethodContext<'_>) -> TraitGoal {
        TraitGoal {
            self_ty: selected_method.selected_self_ty.clone(),
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
