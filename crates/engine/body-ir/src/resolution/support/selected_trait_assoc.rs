//! Projection for associated types written on selected trait methods.
//!
//! Selected trait methods have one extra fact that plain type-ref resolution does not carry:
//! which receiver type selected the trait method. That matters for syntax such as
//! `Self::Item` in `Iterator::collect` or `Iterator::map`, because the `Item` alias has to be
//! read from the receiver impl for that selected `Self` type.
//!
//! In simple words: imagine that you have `foo.iter().map(..)`. You know that `foo` is `Vec<u8>`,
//! and you know that `map()` comes from `Iterator` trait. But `map` talks about `Self::Item`,
//! so what is `Self` here? It might not be the type that you're invoking `map()` on -- trait can
//! be selected through autoderef or the receiver type might not match `Self` overall.
//!
//! To resolve that, we need an extra hop to find a unique trait impl for this method invocation
//! and resolve associated item through it.

use rg_ir_model::{AssocItemId, FunctionRef, ItemOwner, TraitRef, TypeAliasRef, items::TypeRef};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_package_store::PackageStoreError;
use rg_std::ExpectedUnique;
use rg_ty::{
    TraitGoal, TraitSelection, TraitSelectionOptions, TraitSelectionQuery, Ty,
    inference::{InferTy, InferTypeRefProjector, InferenceTable},
};

use crate::resolution::{BodyResolutionContext, TypeRefUseSite};

/// Selected trait-method context needed to interpret `Self::Assoc` syntax.
///
/// Basically, "we think the method comes from this trait, and we have this
/// receiver type at call site", e.g. for `users.iter().map(...)`
/// - `trait_ref` would correspond to `Iterator`
/// - `selected_self_ty` would correspond to `slice::Iter<'_, User>`
///
/// Note that at this stage we have not done impl matching yet, so Self ty
/// is not necessarily `Foo` from `impl Iterator for Foo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SelectedTraitMethodContext<'a> {
    trait_ref: TraitRef,
    selected_self_ty: &'a Ty,
}

impl<'a> SelectedTraitMethodContext<'a> {
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

/// Result of projecting one associated type through a selected receiver impl.
///
/// Roughly "we have done trait solving and here's what we found together with
/// evidence".
pub(crate) struct SelectedTraitAssocProjection {
    ty: InferTy,
    table: InferenceTable,
}

impl SelectedTraitAssocProjection {
    pub(crate) fn into_parts(self) -> (InferTy, InferenceTable) {
        (self.ty, self.table)
    }
}

/// Projects `Self::Assoc` through the unique impl selected by a trait method receiver.
// TODO/watch: this entity is basically two things glued together: probe an impl, and then
// fetch an associated item from it. It kinda feels like two separate concepts wanting
// to be born, but right now we have a domain problem that fits the shape, and no demand
// for either of its halved being generic. In the future, if demand for trait probing at
// callsite grows, we potentially want to split this thing rather than reimplement probing
// everywhere.
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
    ) -> Result<Option<SelectedTraitAssocProjection>, PackageStoreError> {
        let goal = TraitGoal {
            self_ty: InferTy::from_ty(selected_method.selected_self_ty),
            trait_ref: selected_method.trait_ref,
            args: Vec::new(),
        };
        let ExpectedUnique::One(selection) =
            self.probe_trait_goal(&goal, table, TraitSelectionOptions::new())?
        else {
            return Ok(None);
        };
        let Some(projected_ty) =
            self.project_associated_type_from_selection(&selection, assoc_name)?
        else {
            return Ok(None);
        };

        Ok(Some(SelectedTraitAssocProjection {
            ty: projected_ty,
            table: selection.table,
        }))
    }

    /// Select the receiver impl using caller-provided trait-selection options.
    pub(crate) fn select_infer_trait_impl(
        &self,
        selected_method: &SelectedTraitMethodContext<'_>,
        table: &InferenceTable,
        options: TraitSelectionOptions,
    ) -> Result<Option<TraitSelection>, PackageStoreError> {
        let goal = TraitGoal {
            self_ty: InferTy::from_ty(selected_method.selected_self_ty),
            trait_ref: selected_method.trait_ref,
            args: Vec::new(),
        };
        let ExpectedUnique::One(selection) = self.probe_trait_goal(&goal, table, options)? else {
            return Ok(None);
        };
        Ok(Some(selection))
    }

    /// Project an associated type into a stable concrete type for non-mutating callers.
    pub(crate) fn project_concrete_ty(
        &self,
        selected_method: &SelectedTraitMethodContext<'_>,
        assoc_name: &str,
    ) -> Result<Option<Ty>, PackageStoreError> {
        let table = InferenceTable::new();
        let Some(projection) = self.project_infer_ty(selected_method, assoc_name, &table)? else {
            return Ok(None);
        };
        let (projected_ty, table) = projection.into_parts();
        let projected_ty = table.finalize(&projected_ty);
        if matches!(projected_ty, Ty::Syntax(_)) || projected_ty.has_unknown() {
            return Ok(Some(Ty::Unknown));
        }

        Ok(Some(projected_ty))
    }

    pub(crate) fn project_associated_type_from_selection(
        &self,
        selection: &TraitSelection,
        assoc_name: &str,
    ) -> Result<Option<InferTy>, PackageStoreError> {
        let Some((context, aliased_ty)) =
            self.associated_type_alias_from_selection(selection, assoc_name)?
        else {
            return Ok(None);
        };
        let resolved_ty = self
            .context
            .type_refs(TypeRefUseSite::OwnerContext(context))
            .resolve(&aliased_ty)?;
        Ok(Some(
            InferTypeRefProjector::new(&selection.subst)
                .ty_from_type_ref(&aliased_ty, &resolved_ty),
        ))
    }

    pub(crate) fn associated_type_alias_from_selection(
        &self,
        selection: &TraitSelection,
        assoc_name: &str,
    ) -> Result<Option<(TypePathContext, TypeRef)>, PackageStoreError> {
        let Some(impl_data) = self
            .context
            .item_query()
            .impl_data(selection.trait_impl.impl_ref)?
        else {
            return Ok(None);
        };

        for item in &impl_data.items {
            let AssocItemId::TypeAlias(type_alias_id) = item else {
                continue;
            };
            let type_alias_ref = TypeAliasRef {
                origin: selection.trait_impl.impl_ref.origin,
                id: *type_alias_id,
            };
            let Some(type_alias_data) =
                self.context.item_query().type_alias_data(type_alias_ref)?
            else {
                continue;
            };
            if type_alias_data.name.as_str() != assoc_name {
                continue;
            }
            let Some(aliased_ty) = type_alias_data.signature.aliased_ty() else {
                continue;
            };

            let context = TypePathContext {
                module: impl_data.owner,
                impl_ref: Some(selection.trait_impl.impl_ref),
            };
            return Ok(Some((context, aliased_ty.clone())));
        }

        Ok(None)
    }

    fn probe_trait_goal(
        &self,
        goal: &TraitGoal,
        table: &InferenceTable,
        options: TraitSelectionOptions,
    ) -> Result<ExpectedUnique<TraitSelection>, PackageStoreError> {
        TraitSelectionQuery::with_index(
            self.context.item_paths(),
            self.context.target_items(),
            self.context.semantic_index(),
        )
        .with_options(options)
        .probe(goal, table)
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
