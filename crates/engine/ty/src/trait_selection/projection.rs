//! Associated type projection for `TraitSelectionQuery`.
//!
//! The query root owns candidate enumeration and impl probing. This module owns the second half of
//! projection: once a unique impl is selected, ask Chalk for the associated alias first, then use a
//! narrow project-side fallback for source type-ref shapes the Chalk adapter still declines.

use rg_ir_model::{
    AssocItemId, Path, TraitApplicability, TraitRef, TypeAliasRef, TypePathResolution,
    items::{GenericArg as ItemGenericArg, TypeBound, TypeRef, WherePredicate},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_std::ExpectedUnique;
use rg_text::Name;

use super::{TraitGoal, TraitSelection, TraitSelectionQuery};
use crate::Ty;
use crate::inference::{
    InferGenericArg, InferTy, InferTypeRefProjector, InferTypeSubst, InferenceTable,
};

const ASSOCIATED_TYPE_PROJECTION_DEPTH: usize = 8;

/// Result of normalizing one selected associated type projection.
///
/// The projected type is still in inference form because callers usually want to commit it into an
/// active body table, not immediately collapse unsolved variables to `Ty::Unknown`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssocProjectionResult {
    pub ty: InferTy,
    pub applicability: TraitApplicability,
    pub table: InferenceTable,
}

impl AssocProjectionResult {
    pub fn new(ty: InferTy, applicability: TraitApplicability, table: InferenceTable) -> Self {
        Self {
            ty,
            applicability,
            table,
        }
    }

    pub fn into_parts(self) -> (InferTy, TraitApplicability, InferenceTable) {
        (self.ty, self.applicability, self.table)
    }
}

/// Projected type syntax plus the table/applicability learned while projecting it.
struct ProjectedTypeRef {
    ty: InferTy,
    applicability: TraitApplicability,
    table: InferenceTable,
}

/// Trait path syntax after resolving the trait and projecting its generic args.
#[derive(PartialEq, Eq)]
struct ProjectedTraitRef {
    trait_ref: TraitRef,
    args: Vec<InferGenericArg>,
    applicability: TraitApplicability,
    table: InferenceTable,
}

/// Generic arg syntax after any nested associated projections were applied.
struct ProjectedGenericArg {
    arg: InferGenericArg,
    applicability: TraitApplicability,
    table: InferenceTable,
}

impl<'query, D, I> TraitSelectionQuery<'query, D, I>
where
    D: DefMapSource<Error = I::Error>,
    I: ItemStoreSource<'query>,
{
    /// Normalize a named associated type through the unique impl selected for this trait goal.
    ///
    /// This is the narrow projection API used by higher layers. It deliberately returns `None`
    /// for empty or ambiguous selection, and it carries the trial table so the caller can commit
    /// receiver/header evidence only when the whole projection is useful.
    pub fn normalize_assoc_type(
        &self,
        goal: &TraitGoal,
        assoc_name: &str,
        table: &InferenceTable,
    ) -> Result<Option<AssocProjectionResult>, I::Error> {
        self.normalize_assoc_type_with_depth(
            goal,
            assoc_name,
            table,
            ASSOCIATED_TYPE_PROJECTION_DEPTH,
        )
    }

    fn normalize_assoc_type_with_depth(
        &self,
        goal: &TraitGoal,
        assoc_name: &str,
        table: &InferenceTable,
        remaining_depth: usize,
    ) -> Result<Option<AssocProjectionResult>, I::Error> {
        if let Some(projection) =
            Self::project_associated_type_from_opaque_bound(goal, assoc_name, table)
        {
            return Ok(Some(projection));
        }

        let ExpectedUnique::One(selection) = self.probe(goal, table)? else {
            return Ok(None);
        };
        let Some(impl_data) = self
            .target_items
            .items()
            .impl_data(selection.trait_impl.impl_ref)?
        else {
            return Ok(None);
        };
        let context = TypePathContext {
            module: impl_data.owner,
            impl_ref: Some(selection.trait_impl.impl_ref),
        };
        if let Some(mut projection) = self.cache.normalize_assoc_type(
            &self.item_paths,
            &self.target_items,
            context,
            goal,
            assoc_name,
            &selection.table,
        )? {
            projection.applicability = selection.applicability.and(projection.applicability);
            return Ok(Some(projection));
        }

        if remaining_depth == 0 {
            return Ok(None);
        }

        // Keep the old project-side alias reader as a bridge for syntax that the Chalk adapter
        // intentionally declines, for example value-side type refs containing source syntax we do
        // not lower yet. The solver is still the first owner for the supported projection path.
        let Some(projection) =
            self.project_associated_type_from_selection(&selection, assoc_name, remaining_depth)?
        else {
            return Ok(None);
        };

        Ok(Some(projection))
    }

    fn project_associated_type_from_opaque_bound(
        goal: &TraitGoal,
        assoc_name: &str,
        table: &InferenceTable,
    ) -> Option<AssocProjectionResult> {
        let InferTy::Opaque { bounds } = &goal.self_ty else {
            return None;
        };

        // An `impl Trait<Assoc = Ty>` type hides the concrete self type, but the bound itself is a
        // precise projection fact. Treat it as the base case for recursive projection through
        // blanket impls such as `impl<I: Iterator> IntoIterator for I { type Item = I::Item; }`.
        let mut candidates = ExpectedUnique::new();
        for bound in bounds {
            if bound.trait_ref != goal.trait_ref {
                continue;
            }

            let Some(bound_table) =
                Self::match_opaque_bound_goal_args(table, &bound.args, &goal.args)
            else {
                continue;
            };
            for arg in &bound.args {
                let InferGenericArg::AssocType { name, ty: Some(ty) } = arg else {
                    continue;
                };
                if name.as_str() != assoc_name || ty.has_unknown_or_syntax() {
                    continue;
                }

                let projection = AssocProjectionResult {
                    ty: bound_table.canonicalize(ty),
                    applicability: TraitApplicability::Yes,
                    table: bound_table.clone(),
                };
                candidates.push(projection);
            }
        }

        candidates.into_option()
    }

    fn match_opaque_bound_goal_args(
        table: &InferenceTable,
        bound_args: &[InferGenericArg],
        goal_args: &[InferGenericArg],
    ) -> Option<InferenceTable> {
        let mut table = table.clone();
        let mut goal_args = goal_args.iter();
        for bound_arg in bound_args {
            if matches!(bound_arg, InferGenericArg::AssocType { .. }) {
                continue;
            }
            let goal_arg = goal_args.next()?;
            if !Self::match_opaque_bound_goal_arg(&mut table, bound_arg, goal_arg) {
                return None;
            }
        }
        if goal_args.next().is_some() {
            return None;
        }

        Some(table)
    }

    fn match_opaque_bound_goal_arg(
        table: &mut InferenceTable,
        bound_arg: &InferGenericArg,
        goal_arg: &InferGenericArg,
    ) -> bool {
        match (bound_arg, goal_arg) {
            (InferGenericArg::Type(bound_ty), InferGenericArg::Type(goal_ty)) => {
                table.try_unify(bound_ty, goal_ty).is_ok()
            }
            (InferGenericArg::Lifetime(lhs), InferGenericArg::Lifetime(rhs)) => lhs == rhs,
            (InferGenericArg::Const(lhs), InferGenericArg::Const(rhs)) => lhs == rhs,
            (
                InferGenericArg::FnTraitArgs {
                    params: bound_params,
                    ret: bound_ret,
                },
                InferGenericArg::FnTraitArgs {
                    params: goal_params,
                    ret: goal_ret,
                },
            ) if bound_params.len() == goal_params.len() => {
                for (bound_param, goal_param) in bound_params.iter().zip(goal_params) {
                    if table.try_unify(bound_param, goal_param).is_err() {
                        return false;
                    }
                }

                table.try_unify(bound_ret, goal_ret).is_ok()
            }
            (InferGenericArg::Unsupported(lhs), InferGenericArg::Unsupported(rhs)) => lhs == rhs,
            _ => false,
        }
    }

    fn project_associated_type_from_selection(
        &self,
        selection: &TraitSelection,
        assoc_name: &str,
        remaining_depth: usize,
    ) -> Result<Option<AssocProjectionResult>, I::Error> {
        if remaining_depth == 0 {
            return Ok(None);
        }
        let Some(impl_data) = self
            .target_items
            .items()
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
            let Some(type_alias_data) = self.item_paths.items().type_alias_data(type_alias_ref)?
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
            let Some(projected) = self.project_associated_type_ref(
                context,
                &selection.subst,
                &selection.table,
                aliased_ty,
                remaining_depth - 1,
            )?
            else {
                return Ok(None);
            };
            return Ok(Some(AssocProjectionResult {
                ty: projected.ty,
                applicability: selection.applicability.and(projected.applicability),
                table: projected.table,
            }));
        }

        Ok(None)
    }

    fn project_associated_type_ref(
        &self,
        context: TypePathContext,
        subst: &InferTypeSubst,
        table: &InferenceTable,
        ty: &TypeRef,
        remaining_depth: usize,
    ) -> Result<Option<ProjectedTypeRef>, I::Error> {
        if let Some((param_name, assoc_name)) = ty.as_type_param_assoc_path() {
            return self.project_type_param_associated_path(
                context,
                subst,
                table,
                param_name,
                assoc_name.as_str(),
                remaining_depth,
            );
        }

        match ty {
            TypeRef::QualifiedAssociatedType {
                self_ty,
                trait_ty: Some(trait_ty),
                assoc_name,
            } => {
                if remaining_depth == 0 {
                    return Ok(None);
                }
                self.project_qualified_associated_type_ref(
                    context,
                    subst,
                    table,
                    self_ty,
                    trait_ty,
                    assoc_name.as_str(),
                    remaining_depth,
                )
            }
            TypeRef::QualifiedAssociatedType { trait_ty: None, .. } => Ok(None),
            TypeRef::Tuple(fields) => {
                let mut table = table.clone();
                let mut applicability = TraitApplicability::Yes;
                let mut projected_fields = Vec::with_capacity(fields.len());
                for field in fields {
                    let Some(field) = self.project_associated_type_ref(
                        context,
                        subst,
                        &table,
                        field,
                        remaining_depth,
                    )?
                    else {
                        return Ok(None);
                    };
                    applicability = applicability.and(field.applicability);
                    table = field.table;
                    projected_fields.push(field.ty);
                }
                Ok(Some(ProjectedTypeRef {
                    ty: InferTy::Tuple(projected_fields),
                    applicability,
                    table,
                }))
            }
            TypeRef::Reference {
                mutability, inner, ..
            } => {
                let Some(inner) = self.project_associated_type_ref(
                    context,
                    subst,
                    table,
                    inner,
                    remaining_depth,
                )?
                else {
                    return Ok(None);
                };
                Ok(Some(ProjectedTypeRef {
                    ty: InferTy::Reference {
                        mutability: *mutability,
                        inner: Box::new(inner.ty),
                    },
                    applicability: inner.applicability,
                    table: inner.table,
                }))
            }
            TypeRef::Slice(inner) => {
                let Some(inner) = self.project_associated_type_ref(
                    context,
                    subst,
                    table,
                    inner,
                    remaining_depth,
                )?
                else {
                    return Ok(None);
                };
                Ok(Some(ProjectedTypeRef {
                    ty: InferTy::Slice(Box::new(inner.ty)),
                    applicability: inner.applicability,
                    table: inner.table,
                }))
            }
            TypeRef::Array { inner, len } => {
                let Some(inner) = self.project_associated_type_ref(
                    context,
                    subst,
                    table,
                    inner,
                    remaining_depth,
                )?
                else {
                    return Ok(None);
                };
                Ok(Some(ProjectedTypeRef {
                    ty: InferTy::Array {
                        inner: Box::new(inner.ty),
                        len: len.clone(),
                    },
                    applicability: inner.applicability,
                    table: inner.table,
                }))
            }
            _ => {
                let (ty, table) =
                    self.project_plain_associated_type_ref(context, subst, table, ty)?;
                Ok(Some(ProjectedTypeRef {
                    ty,
                    applicability: TraitApplicability::Yes,
                    table,
                }))
            }
        }
    }

    fn project_type_param_associated_path(
        &self,
        context: TypePathContext,
        subst: &InferTypeSubst,
        table: &InferenceTable,
        param_name: &Name,
        assoc_name: &str,
        remaining_depth: usize,
    ) -> Result<Option<ProjectedTypeRef>, I::Error> {
        if remaining_depth == 0 {
            return Ok(None);
        }
        let Some(self_ty) = subst.type_param(param_name.as_str()) else {
            return Ok(None);
        };
        let Some(impl_ref) = context.impl_ref else {
            return Ok(None);
        };
        let Some(impl_data) = self.target_items.items().impl_data(impl_ref)? else {
            return Ok(None);
        };

        // `I::Item` is only useful when the selected impl tells us which trait bound owns `Item`.
        // Keep this unique: if two bounds could own the same associated type name, guessing would
        // leak invented evidence into projection.
        let mut selected_traits = ExpectedUnique::new();
        for param in &impl_data.generics.types {
            if param.name != *param_name {
                continue;
            }
            for bound in &param.bounds {
                let Some(projected_trait) = self.project_type_param_assoc_bound(
                    context,
                    subst,
                    table,
                    bound,
                    assoc_name,
                    remaining_depth,
                )?
                else {
                    continue;
                };
                selected_traits.push(projected_trait);
            }
        }
        for predicate in &impl_data.generics.where_predicates {
            let WherePredicate::Type { ty, bounds } = predicate else {
                continue;
            };
            if ty.type_param_name().as_ref() != Some(param_name) {
                continue;
            }
            for bound in bounds {
                let Some(projected_trait) = self.project_type_param_assoc_bound(
                    context,
                    subst,
                    table,
                    bound,
                    assoc_name,
                    remaining_depth,
                )?
                else {
                    continue;
                };
                selected_traits.push(projected_trait);
            }
        }

        let Some(selected_trait) = selected_traits.into_option() else {
            return Ok(None);
        };
        let goal = TraitGoal {
            self_ty,
            trait_ref: selected_trait.trait_ref,
            args: selected_trait.args,
        };
        let Some(projection) = self.normalize_assoc_type_with_depth(
            &goal,
            assoc_name,
            &selected_trait.table,
            remaining_depth - 1,
        )?
        else {
            return Ok(None);
        };

        Ok(Some(ProjectedTypeRef {
            ty: projection.ty,
            applicability: selected_trait.applicability.and(projection.applicability),
            table: projection.table,
        }))
    }

    fn project_type_param_assoc_bound(
        &self,
        context: TypePathContext,
        subst: &InferTypeSubst,
        table: &InferenceTable,
        bound: &TypeBound,
        assoc_name: &str,
        remaining_depth: usize,
    ) -> Result<Option<ProjectedTraitRef>, I::Error> {
        let TypeBound::Trait(trait_ty) = bound else {
            return Ok(None);
        };
        let Some(projected_trait) =
            self.project_qualified_trait_ref(context, subst, table, trait_ty, remaining_depth)?
        else {
            return Ok(None);
        };
        if !self.trait_declares_assoc_type(projected_trait.trait_ref, assoc_name)? {
            return Ok(None);
        }

        Ok(Some(projected_trait))
    }

    fn trait_declares_assoc_type(
        &self,
        trait_ref: TraitRef,
        assoc_name: &str,
    ) -> Result<bool, I::Error> {
        let Some(trait_data) = self.target_items.items().trait_data(trait_ref)? else {
            return Ok(false);
        };
        for item in &trait_data.items {
            let AssocItemId::TypeAlias(type_alias_id) = item else {
                continue;
            };
            let type_alias_ref = TypeAliasRef {
                origin: trait_ref.origin,
                id: *type_alias_id,
            };
            let Some(type_alias_data) = self.item_paths.items().type_alias_data(type_alias_ref)?
            else {
                continue;
            };
            if type_alias_data.name.as_str() == assoc_name {
                return Ok(true);
            }
        }

        Ok(false)
    }

    #[allow(clippy::too_many_arguments)]
    fn project_qualified_associated_type_ref(
        &self,
        context: TypePathContext,
        subst: &InferTypeSubst,
        table: &InferenceTable,
        self_ty: &TypeRef,
        trait_ty: &TypeRef,
        assoc_name: &str,
        remaining_depth: usize,
    ) -> Result<Option<ProjectedTypeRef>, I::Error> {
        let Some(self_ty) =
            self.project_associated_type_ref(context, subst, table, self_ty, remaining_depth)?
        else {
            return Ok(None);
        };
        let Some(trait_projection) = self.project_qualified_trait_ref(
            context,
            subst,
            &self_ty.table,
            trait_ty,
            remaining_depth,
        )?
        else {
            return Ok(None);
        };
        let applicability = self_ty.applicability.and(trait_projection.applicability);

        let goal = TraitGoal {
            self_ty: self_ty.ty,
            trait_ref: trait_projection.trait_ref,
            args: trait_projection.args,
        };
        let Some(projection) = self.normalize_assoc_type_with_depth(
            &goal,
            assoc_name,
            &trait_projection.table,
            remaining_depth,
        )?
        else {
            return Ok(None);
        };

        Ok(Some(ProjectedTypeRef {
            ty: projection.ty,
            applicability: applicability.and(projection.applicability),
            table: projection.table,
        }))
    }

    fn project_qualified_trait_ref(
        &self,
        context: TypePathContext,
        subst: &InferTypeSubst,
        table: &InferenceTable,
        trait_ty: &TypeRef,
        remaining_depth: usize,
    ) -> Result<Option<ProjectedTraitRef>, I::Error> {
        let TypeRef::Path(path) = trait_ty else {
            return Ok(None);
        };
        let Some(trait_ref) = self.resolve_trait_path_or_unique_visible_name(context, path)? else {
            return Ok(None);
        };
        let Some(segment) = path.segments.last() else {
            return Ok(None);
        };
        let mut table = table.clone();
        let mut applicability = TraitApplicability::Yes;
        let mut args = Vec::with_capacity(segment.args.len());
        for arg in &segment.args {
            let Some(projected_arg) =
                self.project_associated_generic_arg(context, subst, &table, arg, remaining_depth)?
            else {
                return Ok(None);
            };
            applicability = applicability.and(projected_arg.applicability);
            table = projected_arg.table;
            args.push(projected_arg.arg);
        }

        Ok(Some(ProjectedTraitRef {
            trait_ref,
            args,
            applicability,
            table,
        }))
    }

    fn resolve_trait_path_or_unique_visible_name(
        &self,
        context: TypePathContext,
        path: &rg_ir_model::items::TypePath,
    ) -> Result<Option<TraitRef>, I::Error> {
        if let TypePathResolution::Trait(trait_ref) = self
            .item_paths
            .resolve_type_path(context, &Path::from_type_path(path))?
        {
            return Ok(Some(trait_ref));
        }
        let Some(name) = path.single_name() else {
            return Ok(None);
        };

        // Some test and generated contexts can carry a trait path that does not resolve through
        // the ordinary source module, while the trait itself is still visible in the target. Keep
        // that fallback unique and explicit instead of accepting whichever same-named trait we
        // happen to see first.
        //
        // TODO: gate this escape hatch to resolver-generated paths once those contexts carry an
        // explicit marker. In ordinary source code, a missing import should not silently become
        // evidence from a same-named visible trait.
        let mut traits = ExpectedUnique::new();
        for store in self.target_items.visible_stores()? {
            for (trait_ref, trait_data) in store.traits_with_refs() {
                if trait_data.name.as_str() == name.as_str() {
                    traits.push(trait_ref);
                }
            }
        }

        Ok(traits.into_option())
    }

    fn project_associated_generic_arg(
        &self,
        context: TypePathContext,
        subst: &InferTypeSubst,
        table: &InferenceTable,
        arg: &ItemGenericArg,
        remaining_depth: usize,
    ) -> Result<Option<ProjectedGenericArg>, I::Error> {
        match arg {
            ItemGenericArg::Type(ty) => {
                let Some(projected) =
                    self.project_associated_type_ref(context, subst, table, ty, remaining_depth)?
                else {
                    return Ok(None);
                };
                Ok(Some(ProjectedGenericArg {
                    arg: InferGenericArg::Type(Box::new(projected.ty)),
                    applicability: projected.applicability,
                    table: projected.table,
                }))
            }
            ItemGenericArg::Lifetime(lifetime) => Ok(Some(ProjectedGenericArg {
                arg: InferGenericArg::Lifetime(lifetime.clone()),
                applicability: TraitApplicability::Yes,
                table: table.clone(),
            })),
            ItemGenericArg::Const(value) => Ok(Some(ProjectedGenericArg {
                arg: InferGenericArg::Const(value.clone()),
                applicability: TraitApplicability::Yes,
                table: table.clone(),
            })),
            ItemGenericArg::FnTraitArgs { .. }
            | ItemGenericArg::AssocType { .. }
            | ItemGenericArg::Unsupported(_) => Ok(None),
        }
    }

    fn project_plain_associated_type_ref(
        &self,
        context: TypePathContext,
        subst: &InferTypeSubst,
        table: &InferenceTable,
        ty: &TypeRef,
    ) -> Result<(InferTy, InferenceTable), I::Error> {
        let type_subst = subst.finalize_type_subst(table);
        let resolved_ty =
            self.item_paths
                .resolve_type_ref(ty, context, Ty::syntax(ty.clone()), &type_subst)?;
        Ok((
            InferTypeRefProjector::new(subst).ty_from_type_ref(ty, &resolved_ty),
            table.clone(),
        ))
    }
}
