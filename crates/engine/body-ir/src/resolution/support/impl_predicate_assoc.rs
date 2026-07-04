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
    AssocItemId, TraitApplicability, TraitRef, TypeAliasRef,
    hir::items::ImplData,
    items::{GenericParams, TypeBound, TypeRef},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_package_store::PackageStoreError;
use rg_std::ExpectedUnique;
use rg_text::Name;
use rg_ty::{
    AssocProjectionResult, TraitGoal, TraitSelection, TraitSelectionCache, TraitSelectionOptions,
    TraitSelectionQuery,
    inference::{InferGenericArg, InferTy, InferTypeRefProjector, InferenceTable},
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

/// Non-callable predicate that can normalize `T::Assoc` inside a concrete impl alias.
///
/// Example: when projecting `Adapter<S>::Output = S::Item`, the `S: Source` predicate becomes a
/// support goal. The support is marked as used only if the alias actually needed `S::Item`, so
/// unrelated predicates are not accidentally accepted.
pub(crate) struct ProjectionSupport {
    param_name: Name,
    goal: TraitGoal,
    used: bool,
}

impl ProjectionSupport {
    pub(crate) fn new(param_name: Name, goal: TraitGoal) -> Self {
        Self {
            param_name,
            goal,
            used: false,
        }
    }

    pub(crate) fn unused(&self) -> bool {
        !self.used
    }
}

/// Find the single support predicate that can answer one `T::Assoc` projection.
///
/// Both shallow projection and body-obligation projection need the same bookkeeping: match support
/// by impl type parameter, optionally require a qualified trait and args, reject ambiguity, and
/// mark the support as used only after the caller's projection succeeds. The caller still supplies
/// the projection closure, so recursive local projection and inference-table normalization remain
/// separate policies.
pub(crate) fn project_unique_support_assoc<T, E>(
    supports: &mut [ProjectionSupport],
    param_name: &Name,
    qualified_trait: Option<(TraitRef, &[InferGenericArg])>,
    mut project: impl FnMut(&TraitGoal) -> Result<Option<T>, E>,
) -> Result<Option<T>, E> {
    let mut candidate = None;

    for (support_idx, support) in supports.iter().enumerate() {
        if support.param_name.as_str() != param_name.as_str() {
            continue;
        }
        if let Some((trait_ref, args)) = qualified_trait
            && (support.goal.trait_ref != trait_ref || support.goal.args != args)
        {
            continue;
        }

        let Some(projection) = project(&support.goal)? else {
            continue;
        };
        if candidate.is_some() {
            return Ok(None);
        }
        candidate = Some((support_idx, projection));
    }

    let Some((support_idx, projection)) = candidate else {
        return Ok(None);
    };
    supports[support_idx].used = true;
    Ok(Some(projection))
}

/// Projects associated aliases using the simple impl predicates body resolution understands.
pub(crate) struct ImplPredicateAssocProjector<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
    trait_selection_cache: TraitSelectionCache,
}

impl<'query, D, I> ImplPredicateAssocProjector<'query, D, I>
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
    ///
    /// If the selected impl has no caller-owned predicates, this returns `None` so the caller can
    /// use the shared type-layer projection API for direct associated aliases. Callable predicates
    /// still count as body-local work because they can introduce impl-only type parameters such as
    /// `B` in `Map<I, F>::Item = B`.
    pub(crate) fn project_goal_through_impl_predicates(
        &self,
        goal: &TraitGoal,
        assoc_name: &str,
        table: &InferenceTable,
    ) -> Result<Option<AssocProjectionResult>, PackageStoreError> {
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
        .with_cache(self.trait_selection_cache.clone())
        .with_options(options)
        .probe(goal, table)
    }

    fn project_goal_through_impl_predicates_inner(
        &self,
        goal: &TraitGoal,
        assoc_name: &str,
        table: &InferenceTable,
        remaining_depth: usize,
    ) -> Result<Option<AssocProjectionResult>, PackageStoreError> {
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
        if !Self::has_body_local_impl_predicates(&impl_data.generics) {
            return Ok(None);
        }
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
        let mut applicability = selection.applicability;
        let Some(projected_ty) = self.project_impl_ty(
            &mut selection,
            &mut supports,
            &resolver,
            &aliased_ty,
            remaining_depth,
            &mut applicability,
        )?
        else {
            return Ok(None);
        };
        // Every accepted support predicate must be relevant to the alias. If an impl needs extra
        // non-callable evidence we did not consume, the result would be too confident.
        if supports.iter().any(ProjectionSupport::unused) {
            return Ok(None);
        }

        Ok(Some(AssocProjectionResult::new(
            projected_ty,
            applicability,
            selection.table,
        )))
    }

    fn has_body_local_impl_predicates(generics: &GenericParams) -> bool {
        generics.types.iter().any(|param| !param.bounds.is_empty())
            || !generics.where_predicates.is_empty()
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
            return Ok(Some(ProjectionSupport::new(
                param_name,
                TraitGoal {
                    self_ty,
                    trait_ref,
                    args,
                },
            )));
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
        applicability: &mut TraitApplicability,
    ) -> Result<Option<InferTy>, PackageStoreError> {
        let subst = selection.subst.clone();
        let mut associated_ty = |param_name: &Name, qualified_trait, assoc_name: &Name| {
            if let Some((trait_ref, args)) = qualified_trait {
                self.project_impl_qualified_generic_associated_ty(
                    selection,
                    supports,
                    param_name,
                    trait_ref,
                    args,
                    assoc_name,
                    remaining_depth,
                    applicability,
                )
            } else {
                self.project_impl_generic_associated_ty(
                    selection,
                    supports,
                    param_name,
                    assoc_name,
                    remaining_depth,
                    applicability,
                )
            }
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
        applicability: &mut TraitApplicability,
    ) -> Result<Option<InferTy>, PackageStoreError> {
        let Some(projection) = project_unique_support_assoc(supports, param_name, None, |goal| {
            self.project_support_goal_assoc_type(
                goal,
                assoc_name.as_str(),
                &selection.table,
                remaining_depth,
            )
        })?
        else {
            return Ok(None);
        };
        *applicability = applicability.and(projection.applicability);
        selection.table = projection.table;
        Ok(Some(projection.ty))
    }

    #[allow(clippy::too_many_arguments)]
    fn project_impl_qualified_generic_associated_ty(
        &self,
        selection: &mut TraitSelection,
        supports: &mut [ProjectionSupport],
        param_name: &Name,
        trait_ref: TraitRef,
        args: Vec<InferGenericArg>,
        assoc_name: &Name,
        remaining_depth: usize,
        applicability: &mut TraitApplicability,
    ) -> Result<Option<InferTy>, PackageStoreError> {
        let Some(projection) =
            project_unique_support_assoc(supports, param_name, Some((trait_ref, &args)), |goal| {
                self.project_support_goal_assoc_type(
                    goal,
                    assoc_name.as_str(),
                    &selection.table,
                    remaining_depth,
                )
            })?
        else {
            return Ok(None);
        };
        *applicability = applicability.and(projection.applicability);
        selection.table = projection.table;
        Ok(Some(projection.ty))
    }

    fn project_support_goal_assoc_type(
        &self,
        goal: &TraitGoal,
        assoc_name: &str,
        table: &InferenceTable,
        remaining_depth: usize,
    ) -> Result<Option<AssocProjectionResult>, PackageStoreError> {
        if remaining_depth == 0 {
            return Ok(None);
        }

        // A support goal can itself be another adapter-like impl whose alias depends on
        // caller-owned predicates. Try that local bridge first so predicates such as
        // `S: Source` remain visible to nested `S::Item` projections.
        if let Some(projection) = self.project_goal_through_impl_predicates_inner(
            goal,
            assoc_name,
            table,
            remaining_depth - 1,
        )? {
            return Ok(Some(projection));
        }

        // If the support impl has no body-local predicate work, the associated alias belongs to
        // the shared type layer. This is the common endpoint for projections like
        // `<slice::Iter<T> as Iterator>::Item = T`.
        TraitSelectionQuery::with_index(
            self.context.item_paths(),
            self.context.target_items(),
            self.context.semantic_index(),
        )
        .with_cache(self.trait_selection_cache.clone())
        .normalize_assoc_type(goal, assoc_name, table)
    }
}
