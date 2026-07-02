//! Associated type projection through impl predicates.
//!
//! The type layer can normalize direct impl aliases, but some impls define an associated type
//! through another impl parameter:
//!
//! ```text
//! impl<S: Source> Produces for Adapter<S> {
//!     type Output = S::Item;
//! }
//! ```
//!
//! This helper handles that body-local bridge. It selects the outer impl, gathers the simple
//! support predicates that can prove `S::Item`, and recursively projects those nested goals. It
//! intentionally stays shallow: callable predicates are skipped here because closure evidence is
//! owned by the body-obligation pass.

use rg_ir_model::{
    AssocItemId, TypeAliasRef,
    hir::items::ImplData,
    items::{GenericParams, TypeBound, TypeRef},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_package_store::PackageStoreError;
use rg_std::ExpectedUnique;
use rg_text::Name;
use rg_ty::{
    TraitGoal, TraitSelection, TraitSelectionOptions, TraitSelectionQuery,
    inference::{InferTy, InferTypeRefProjector, InferenceTable},
};

use crate::resolution::{BodyResolutionContext, TypeRefUseSite, query::TypeRefResolutionQuery};

use super::{
    BodyTypeRefProjector, CallableTypeRefExpectation, ImplPredicateSubject,
    impl_projection_predicates,
};

// Recursive associated aliases are useful, but an accidental cycle must stay an unknown projection
// instead of growing the Rust stack. Eight steps is enough for ordinary nested projections while
// still being small enough to make runaway projection obviously bounded.
const IMPL_PREDICATE_ASSOC_PROJECTION_DEPTH: usize = 8;

/// Result of projecting one associated type through impl-predicate evidence.
///
/// The projected type is still in inference form because callers usually want to commit it into an
/// active body table, not immediately collapse unsolved variables to `Ty::Unknown`.
pub(crate) struct ImplPredicateAssocProjection {
    ty: InferTy,
    table: InferenceTable,
}

impl ImplPredicateAssocProjection {
    pub(crate) fn new(ty: InferTy, table: InferenceTable) -> Self {
        Self { ty, table }
    }

    pub(crate) fn into_parts(self) -> (InferTy, InferenceTable) {
        (self.ty, self.table)
    }
}

/// Non-callable predicate that can normalize `T::Assoc` inside a concrete impl alias.
///
/// Example: when projecting `Adapter<S>::Output = S::Item`, the `S: Source` predicate becomes a
/// support goal. The support is marked as used only if the alias actually needed `S::Item`, so
/// unrelated predicates are not accidentally accepted.
struct ProjectionSupport {
    param_name: Name,
    goal: TraitGoal,
    used: bool,
}

/// Projects associated aliases using the simple impl predicates body resolution understands.
pub(crate) struct ImplPredicateAssocProjector<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> ImplPredicateAssocProjector<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Select one impl for a trait goal using caller-provided trait-selection options.
    pub(crate) fn select_trait_impl(
        &self,
        goal: &TraitGoal,
        table: &InferenceTable,
        options: TraitSelectionOptions,
    ) -> Result<Option<TraitSelection>, PackageStoreError> {
        let ExpectedUnique::One(selection) = self.probe_trait_goal(goal, table, options)? else {
            return Ok(None);
        };
        Ok(Some(selection))
    }

    /// Find the written associated alias body for an already selected impl.
    ///
    /// The returned context is the impl owner's context, not the call-site context. Alias bodies
    /// such as `type Output = S::Item` must resolve names exactly where the impl was written.
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

    /// Project a trait goal's associated type using only support predicates understood locally.
    ///
    /// This intentionally does not solve callable return variables. It is for nested support goals
    /// such as `Adapter<S>::Output`, where `S: Source` is enough to normalize the alias, and for
    /// early concrete call-signature projection where mutating closure inference is not available.
    pub(crate) fn project_goal_through_impl_predicates(
        &self,
        goal: &TraitGoal,
        assoc_name: &str,
        table: &InferenceTable,
    ) -> Result<Option<ImplPredicateAssocProjection>, PackageStoreError> {
        self.project_goal_through_impl_predicates_inner(
            goal,
            assoc_name,
            table,
            IMPL_PREDICATE_ASSOC_PROJECTION_DEPTH,
        )
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

    fn project_goal_through_impl_predicates_inner(
        &self,
        goal: &TraitGoal,
        assoc_name: &str,
        table: &InferenceTable,
        remaining_depth: usize,
    ) -> Result<Option<ImplPredicateAssocProjection>, PackageStoreError> {
        if remaining_depth == 0 {
            return Ok(None);
        }

        // 1. Select the outer impl by header only, but in the policy mode where the caller promises
        // to inspect all supported impl predicates below.
        let Some(mut selection) = self.select_trait_impl(
            goal,
            table,
            TraitSelectionOptions::new().caller_solves_impl_predicates(),
        )?
        else {
            return Ok(None);
        };
        // 2. Give impl-only type params a fresh inference slot. For an impl whose alias is a
        // callable result, `B` may be learned later from a predicate even though it does not appear
        // in the impl self type.
        let Some(impl_data) = self
            .context
            .item_query()
            .impl_data(selection.trait_impl.impl_ref)?
            .cloned()
        else {
            return Ok(None);
        };
        Self::bind_missing_impl_type_params(&mut selection, &impl_data.generics);
        // 3. Read the alias body in the impl context and collect the predicates that can support
        // projections inside that body.
        let Some((context, aliased_ty)) =
            self.associated_type_alias_from_selection(&selection, assoc_name)?
        else {
            return Ok(None);
        };
        let resolver = self
            .context
            .type_refs(TypeRefUseSite::OwnerContext(context));

        let Some(mut supports) = self.projection_supports(&mut selection, &impl_data)? else {
            return Ok(None);
        };
        // 4. Project the alias body using the support goals. Any unsupported associated projection
        // keeps the whole alias unknown.
        let Some(projected_ty) = self.project_impl_ty(
            &mut selection,
            &mut supports,
            &resolver,
            &aliased_ty,
            remaining_depth,
        )?
        else {
            return Ok(None);
        };
        // Every accepted support predicate must be relevant to the alias. If an impl needs extra
        // non-callable evidence we did not consume, the result would be too confident.
        if supports.iter().any(|support| !support.used) {
            return Ok(None);
        }

        Ok(Some(ImplPredicateAssocProjection::new(
            projected_ty,
            selection.table,
        )))
    }

    fn bind_missing_impl_type_params(selection: &mut TraitSelection, generics: &GenericParams) {
        for param in &generics.types {
            if selection.subst.type_param(param.name.as_str()).is_some() {
                continue;
            }
            let ty = selection.table.new_type_var();
            selection
                .subst
                .push(&mut selection.table, param.name.clone(), ty);
        }
    }

    /// Convert impl predicates into the support goals used by alias projection.
    ///
    /// Callable-only predicates are skipped here: they are useful for the final body-obligation
    /// path, but this generic projector cannot inspect closure bodies. Non-callable predicates
    /// must become support goals, otherwise we do not know how to prove projections that depend on
    /// them.
    fn projection_supports(
        &self,
        selection: &mut TraitSelection,
        impl_data: &ImplData,
    ) -> Result<Option<Vec<ProjectionSupport>>, PackageStoreError> {
        let context = TypePathContext {
            module: impl_data.owner,
            impl_ref: Some(selection.trait_impl.impl_ref),
        };
        let resolver = self
            .context
            .type_refs(TypeRefUseSite::OwnerContext(context));

        let Some(predicates) = impl_projection_predicates(&impl_data.generics) else {
            return Ok(None);
        };
        let mut supports = Vec::new();
        for predicate in predicates {
            if predicate.bounds.is_empty() {
                return Ok(None);
            }
            if Self::all_callable_bounds(predicate.bounds) {
                continue;
            }

            let Some(support) = Self::projection_support(
                selection,
                &resolver,
                &predicate.subject,
                predicate.bounds,
            )?
            else {
                return Ok(None);
            };
            supports.push(support);
        }

        Ok(Some(supports))
    }

    fn all_callable_bounds(bounds: &[TypeBound]) -> bool {
        bounds.iter().all(|bound| {
            let TypeBound::Trait(bound_ty) = bound else {
                return false;
            };
            CallableTypeRefExpectation::from_fn_trait_bound(bound_ty).is_some()
        })
    }

    /// Turn a simple predicate like `S: Source` into a goal for later `S::Item` projection.
    ///
    /// The supported shape is intentionally narrow: a single trait bound whose subject is a known
    /// impl type parameter. More complex predicates stay unsupported so this helper does not become
    /// a hidden trait solver.
    fn projection_support(
        selection: &TraitSelection,
        resolver: &TypeRefResolutionQuery<'query, D, I>,
        subject: &ImplPredicateSubject,
        bounds: &[TypeBound],
    ) -> Result<Option<ProjectionSupport>, PackageStoreError> {
        if let Some(param_name) = subject.type_param_name()
            && let Some(self_ty) = selection.subst.type_param(param_name.as_str())
            && let [TypeBound::Trait(bound_ty)] = bounds
            && CallableTypeRefExpectation::from_fn_trait_bound(bound_ty).is_none()
            && let Some((trait_ref, resolved_args)) = resolver.resolve_trait_bound(bound_ty)?
            && let TypeRef::Path(bound_path) = bound_ty
            && let Some(segment) = bound_path.segments.last()
            && segment.args.len() == resolved_args.len()
        {
            let args = segment
                .args
                .iter()
                .zip(&resolved_args)
                .map(|(arg, resolved_arg)| {
                    InferTypeRefProjector::new(&selection.subst)
                        .generic_arg_from_arg(arg, resolved_arg)
                })
                .collect();
            return Ok(Some(ProjectionSupport {
                param_name,
                goal: TraitGoal {
                    self_ty,
                    trait_ref,
                    args,
                },
                used: false,
            }));
        }

        Ok(None)
    }

    /// Project an impl alias body while letting `T::Assoc` ask the support goals for help.
    ///
    /// Ordinary type syntax still goes through `BodyTypeRefProjector`'s fallback path, so
    /// `Option<S::Item>` preserves the `Option` wrapper and only the `S::Item` part needs special
    /// evidence.
    fn project_impl_ty(
        &self,
        selection: &mut TraitSelection,
        supports: &mut [ProjectionSupport],
        resolver: &TypeRefResolutionQuery<'query, D, I>,
        ty: &TypeRef,
        remaining_depth: usize,
    ) -> Result<Option<InferTy>, PackageStoreError> {
        let subst = selection.subst.clone();
        let mut associated_ty = |param_name: &Name, assoc_name: &Name| {
            self.project_impl_generic_associated_ty(
                selection,
                supports,
                param_name,
                assoc_name,
                remaining_depth,
            )
        };
        BodyTypeRefProjector::new(&subst, resolver)
            .with_type_param_associated_ty(&mut associated_ty)
            .ty_if_supported(ty)
    }

    /// Resolve one `T::Assoc` occurrence through the matching support goal.
    ///
    /// If more than one support goal can answer, the projection is ambiguous. If none can answer,
    /// the caller keeps the containing alias unknown.
    fn project_impl_generic_associated_ty(
        &self,
        selection: &mut TraitSelection,
        supports: &mut [ProjectionSupport],
        param_name: &Name,
        assoc_name: &Name,
        remaining_depth: usize,
    ) -> Result<Option<InferTy>, PackageStoreError> {
        let mut candidate = None;

        for (support_idx, support) in supports.iter().enumerate() {
            if support.param_name.as_str() == param_name.as_str()
                && let Some(projection) = self.project_goal_through_impl_predicates_inner(
                    &support.goal,
                    assoc_name.as_str(),
                    &selection.table,
                    remaining_depth - 1,
                )?
            {
                if candidate.is_some() {
                    return Ok(None);
                }
                candidate = Some((support_idx, projection.table, projection.ty));
            }
        }

        let Some((support_idx, projection_table, projected_ty)) = candidate else {
            return Ok(None);
        };
        selection.table = projection_table;
        supports[support_idx].used = true;
        Ok(Some(projected_ty))
    }
}
