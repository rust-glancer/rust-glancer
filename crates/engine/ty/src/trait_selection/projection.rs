//! Associated type projection for `TraitSelectionQuery`.
//!
//! The query root owns candidate enumeration and impl probing. This module owns the second half of
//! projection: once a unique impl is selected, ask Chalk for the associated alias first, then use a
//! narrow project-side fallback for source type-ref shapes the Chalk adapter still declines.

use rg_ir_model::{
    AssocItemId, Path, TraitApplicability, TraitRef, TypeAliasRef, TypePathResolution,
    items::{GenericArg as ItemGenericArg, TypeRef},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_std::ExpectedUnique;

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

struct ProjectedTypeRef {
    ty: InferTy,
    applicability: TraitApplicability,
    table: InferenceTable,
}

struct ProjectedTraitRef {
    trait_ref: TraitRef,
    args: Vec<InferGenericArg>,
    applicability: TraitApplicability,
    table: InferenceTable,
}

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
