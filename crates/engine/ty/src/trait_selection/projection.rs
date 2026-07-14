//! Associated-type and canonical type normalization for `TraitSelectionQuery`.
//!
//! Candidate matching first identifies one impl and preserves its trial inference table. Chalk is
//! the primary projection engine. When Chalk cannot finish a transitive alias, the already-selected
//! impl's canonical semantic value can expose the next projection without returning to `TypeRef`
//! syntax. Recursive normalization then submits that projection to Chalk as an ordinary goal.

use rg_def_map::DefMapSource;
use rg_ir_model::{
    AssocItemId, GenericDefRef, ImplRef, ItemOwner, TraitApplicability, TraitDefRef, TypeAliasRef,
};
use rg_semantic_ir::ItemStoreSource;
use rg_std::ExpectedUnique;

use super::{TraitGoal, TraitSelectionQuery};
use crate::inference::{InferenceSubstitution, InferenceTable};
use crate::{
    AdtTy, AliasTy, FnDefTy, GenericArg, GenericArgs, ItemPathQuery, OpaqueTy, ProjectionTy,
    SemanticSignatureQuery, Substitution, TraitApplication, Ty,
};

/// Result of normalizing one selected associated type projection.
///
/// The projected type is still in inference form because callers usually want to commit it into an
/// active body table, not immediately collapse unsolved variables to `Ty::Unknown`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssocProjectionResult {
    pub ty: Ty,
    pub applicability: TraitApplicability,
    pub table: InferenceTable,
}

impl AssocProjectionResult {
    pub fn new(ty: Ty, applicability: TraitApplicability, table: InferenceTable) -> Self {
        Self {
            ty,
            applicability,
            table,
        }
    }

    pub fn into_parts(self) -> (Ty, TraitApplicability, InferenceTable) {
        (self.ty, self.applicability, self.table)
    }
}

impl<'query, D, I> TraitSelectionQuery<'query, D, I>
where
    D: DefMapSource<Error = I::Error>,
    I: ItemStoreSource<'query>,
{
    /// Normalize a named associated type through the unique impl selected for this trait goal.
    ///
    /// Chalk owns normal projection. Its bounded solver can leave a transitive value such as
    /// `type Item = I::Item` unresolved. In that case we read the same canonical `Ty` stored for
    /// the uniquely selected impl and apply its owner-scoped substitution. This fallback only
    /// reveals another semantic type; it never reinterprets declaration syntax.
    ///
    /// A selected leaf value such as `type Item = T` needs no solver at all: once every impl
    /// parameter is bound, applying the canonical substitution is already the final projection.
    pub fn normalize_assoc_type(
        &self,
        goal: &TraitGoal,
        assoc_name: &str,
        table: &InferenceTable,
    ) -> Result<Option<AssocProjectionResult>, I::Error> {
        let ExpectedUnique::One(selection) = self.probe(goal, table)? else {
            return Ok(None);
        };
        let selected_value = Self::canonical_impl_assoc_value(
            &self.item_paths,
            selection.trait_impl.impl_ref,
            &selection.subst,
            assoc_name,
        )?;
        if let Some(ty) = selected_value.as_ref()
            && !ty.has_projection()
        {
            return Ok(Some(AssocProjectionResult {
                ty: ty.clone(),
                applicability: selection.applicability,
                table: selection.table,
            }));
        }
        if let Some(mut projection) = self.cache.normalize_assoc_type(
            &self.item_paths,
            &self.crate_items,
            goal,
            assoc_name,
            &selection.table,
        )? && !matches!(projection.ty, Ty::Unknown)
        {
            projection.applicability = selection.applicability.and(projection.applicability);
            return Ok(Some(projection));
        }

        let Some(ty) = selected_value else {
            return Ok(None);
        };
        Ok(Some(AssocProjectionResult {
            ty,
            applicability: selection.applicability,
            table: selection.table,
        }))
    }

    /// Normalize every associated projection reachable inside one semantic type.
    ///
    /// Unsupported and cyclic projections stay as aliases. The returned table includes only
    /// evidence from unique successful projections, so a body caller can adopt it atomically.
    pub fn normalize_ty(
        &self,
        ty: &Ty,
        table: &InferenceTable,
    ) -> Result<(Ty, InferenceTable), I::Error> {
        let mut table = table.clone();
        let ty = self.normalize_ty_with_table(ty, &mut table, &mut Vec::new())?;
        Ok((ty, table))
    }

    /// Read one associated value from the selected impl after every impl parameter is known.
    ///
    /// Requiring a full impl substitution keeps params that exist only in unsolved where-clauses
    /// from escaping as declaration-owned `Ty::Param` values. Chalk remains responsible for those
    /// cases; this path is for values whose selected header already supplied all inputs.
    pub(super) fn canonical_impl_assoc_value(
        item_paths: &ItemPathQuery<'query, D, I>,
        impl_ref: ImplRef,
        subst: &InferenceSubstitution,
        assoc_name: &str,
    ) -> Result<Option<Ty>, I::Error> {
        let generics = item_paths
            .generics()
            .generics(GenericDefRef::Impl(impl_ref))?;
        if generics
            .iter_self()
            .any(|param| subst.as_substitution().get(param.param()).is_none())
        {
            return Ok(None);
        }

        let Some(impl_data) = item_paths.items().impl_data(impl_ref)? else {
            return Ok(None);
        };
        for item in &impl_data.items {
            let AssocItemId::TypeAlias(id) = item else {
                continue;
            };
            let alias = TypeAliasRef {
                origin: impl_ref.origin,
                id: *id,
            };
            let Some(data) = item_paths.items().type_alias_data(alias)? else {
                continue;
            };
            if data.name.as_str() != assoc_name {
                continue;
            }
            let Some(ty) = SemanticSignatureQuery::type_alias_ty_from(item_paths, alias)? else {
                return Ok(None);
            };
            return Ok(Some(subst.as_substitution().apply(&ty)));
        }

        Ok(None)
    }

    fn normalize_ty_with_table(
        &self,
        ty: &Ty,
        table: &mut InferenceTable,
        active: &mut Vec<ProjectionTy>,
    ) -> Result<Ty, I::Error> {
        Ok(match ty {
            Ty::Tuple(fields) => Ty::tuple(
                fields
                    .iter()
                    .map(|field| self.normalize_ty_with_table(field, table, active))
                    .collect::<Result<_, _>>()?,
            ),
            Ty::Array { inner, len } => Ty::Array {
                inner: Box::new(self.normalize_ty_with_table(inner, table, active)?),
                len: *len,
            },
            Ty::Slice(inner) => Ty::slice(self.normalize_ty_with_table(inner, table, active)?),
            Ty::Reference {
                lifetime,
                mutability,
                inner,
            } => Ty::reference_with_lifetime(
                *lifetime,
                *mutability,
                self.normalize_ty_with_table(inner, table, active)?,
            ),
            Ty::RawPointer { mutability, inner } => Ty::raw_pointer(
                *mutability,
                self.normalize_ty_with_table(inner, table, active)?,
            ),
            Ty::FnPointer { params, ret } => Ty::fn_pointer(
                params
                    .iter()
                    .map(|param| self.normalize_ty_with_table(param, table, active))
                    .collect::<Result<_, _>>()?,
                self.normalize_ty_with_table(ret, table, active)?,
            ),
            Ty::Adt(ty) => Ty::Adt(AdtTy {
                def: ty.def,
                args: self.normalize_args(&ty.args, table, active)?,
            }),
            Ty::FnDef(function) => Ty::FnDef(FnDefTy {
                def: function.def,
                args: self.normalize_args(&function.args, table, active)?,
            }),
            Ty::Alias(AliasTy::Opaque(opaque)) => Ty::Alias(AliasTy::Opaque(OpaqueTy {
                opaque: opaque.opaque,
                args: self.normalize_args(&opaque.args, table, active)?,
            })),
            Ty::Alias(AliasTy::Projection(alias)) => {
                let alias = ProjectionTy {
                    associated_ty: alias.associated_ty,
                    args: self.normalize_args(&alias.args, table, active)?,
                };
                if active.contains(&alias) {
                    return Ok(Ty::Alias(AliasTy::Projection(alias)));
                }

                // An opaque return type exposes only its declared bounds. Those bounds already
                // contain canonical associated equalities, so `<impl Iterator<Item = T> as
                // Iterator>::Item` can be answered directly without inventing an impl identity.
                if let Some(ty) = self.opaque_assoc_value(&alias)? {
                    active.push(alias);
                    let ty = self.normalize_ty_with_table(&ty, table, active)?;
                    active.pop();
                    return Ok(ty);
                }

                let Some(data) = self
                    .item_paths
                    .items()
                    .type_alias_data(alias.associated_ty)?
                else {
                    return Ok(Ty::Alias(AliasTy::Projection(alias)));
                };
                let ItemOwner::Trait(trait_id) = data.owner else {
                    return Ok(Ty::Alias(AliasTy::Projection(alias)));
                };
                let goal = TraitGoal {
                    application: TraitApplication {
                        def: TraitDefRef {
                            origin: alias.associated_ty.origin,
                            id: trait_id,
                        },
                        args: alias.args.clone(),
                    },
                    associated_types: Vec::new(),
                };

                // Associated values can form cycles across multiple aliases. Stop at the first
                // repeated semantic projection and keep that alias visible to the caller.
                active.push(alias.clone());
                let normalized = self.normalize_assoc_type(&goal, data.name.as_str(), table)?;
                let ty = if let Some(normalized) = normalized {
                    let (ty, _applicability, normalized_table) = normalized.into_parts();
                    *table = normalized_table;
                    self.normalize_ty_with_table(&ty, table, active)?
                } else {
                    Ty::Alias(AliasTy::Projection(alias))
                };
                active.pop();
                ty
            }
            Ty::Unit
            | Ty::Never
            | Ty::Primitive(_)
            | Ty::Param(_)
            | Ty::Closure(_)
            | Ty::Unknown
            | Ty::InferVar { .. } => ty.clone(),
        })
    }

    fn normalize_args(
        &self,
        args: &GenericArgs,
        table: &mut InferenceTable,
        active: &mut Vec<ProjectionTy>,
    ) -> Result<GenericArgs, I::Error> {
        args.iter()
            .map(|arg| {
                Ok(match arg {
                    GenericArg::Type(ty) => {
                        GenericArg::Type(Box::new(self.normalize_ty_with_table(ty, table, active)?))
                    }
                    GenericArg::Lifetime(_) | GenericArg::Const(_) => arg.clone(),
                })
            })
            .collect()
    }

    /// Read an associated equality from the bounds attached to an opaque `Self` type.
    fn opaque_assoc_value(&self, alias: &ProjectionTy) -> Result<Option<Ty>, I::Error> {
        let Some(Ty::Alias(AliasTy::Opaque(opaque))) =
            alias.args.first().and_then(GenericArg::as_ty)
        else {
            return Ok(None);
        };
        let bounds = SemanticSignatureQuery::opaque_bounds_for_owner_from(
            &self.item_paths,
            opaque.opaque.owner,
        )?;
        let Some((_, bounds)) = bounds
            .into_iter()
            .find(|(candidate, _)| candidate.opaque == opaque.opaque)
        else {
            return Ok(None);
        };
        let generics = self.item_paths.generics().generics(opaque.opaque.owner)?;
        let subst = Substitution::from_args(&generics, &opaque.args);

        for bound in bounds {
            let bound = subst.apply_trait_ref(&bound);
            if bound.application.args != alias.args {
                continue;
            }
            if let Some(binding) = bound
                .associated_types
                .into_iter()
                .find(|binding| binding.associated_ty == alias.associated_ty)
            {
                return Ok(Some(binding.ty));
            }
        }
        Ok(None)
    }
}
