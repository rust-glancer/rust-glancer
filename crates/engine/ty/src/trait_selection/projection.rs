//! Associated-type and canonical type normalization for `TraitSelectionQuery`.
//!
//! Chalk is the projection engine for both concrete impls and environment evidence such as an
//! opaque type's declared bounds. Native candidate matching can identify one concrete impl and
//! preserve its trial inference table, but projection does not require that impl: recursive
//! normalization feeds every semantic projection back through the solver boundary.

use rg_def_map::DefMapSource;
use rg_ir_model::{ItemOwner, TraitApplicability, TraitDefRef};
use rg_semantic_ir::ItemStoreSource;
use rg_std::ExpectedUnique;

use super::{ChalkOutcome, TraitGoal, TraitSelectionQuery};
use crate::inference::InferenceTable;
use crate::{
    AdtTy, AliasTy, FnDefTy, GenericArg, GenericArgs, OpaqueTy, ProjectionTy, TraitApplication, Ty,
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
    pub(crate) fn into_parts(self) -> (Ty, TraitApplicability, InferenceTable) {
        (self.ty, self.applicability, self.table)
    }
}

impl<'query, D, I> TraitSelectionQuery<'query, D, I>
where
    D: DefMapSource<Error = I::Error>,
    I: ItemStoreSource<'query>,
{
    /// Normalize a named associated type through Chalk's evidence for this trait goal.
    ///
    /// A unique native impl lets the adapter instantiate the associated value already stored in
    /// its Chalk datum. Opaque bounds provide the same kind of exact program evidence without an
    /// impl identity. Remaining goals enter the bounded solver forest. If none of those paths can
    /// model or decode the projection, the query returns no semantic fact.
    pub fn normalize_assoc_type(
        &self,
        goal: &TraitGoal,
        assoc_name: &str,
        table: &InferenceTable,
    ) -> Result<Option<AssocProjectionResult>, I::Error> {
        let Some(mut projection) = self.normalize_assoc_type_once(goal, assoc_name, table)? else {
            return Ok(None);
        };
        let mut table = projection.table;
        projection.ty =
            self.normalize_ty_with_table(&projection.ty, &mut table, &mut Vec::new())?;
        projection.table = table;
        Ok(Some(projection))
    }

    /// Project one associated value without recursively normalizing aliases inside that value.
    fn normalize_assoc_type_once(
        &self,
        goal: &TraitGoal,
        assoc_name: &str,
        table: &InferenceTable,
    ) -> Result<Option<AssocProjectionResult>, I::Error> {
        let selection = match self.probe(goal, table)? {
            ExpectedUnique::One(selection) => Some(selection),
            ExpectedUnique::Empty | ExpectedUnique::Ambiguous => None,
        };
        let selection_table = selection
            .as_ref()
            .map(|selection| &selection.table)
            .unwrap_or(table);
        let projection = self.context.trait_selection().normalize_assoc_type(
            self.context.item_paths(),
            self.context.crate_items(),
            goal,
            assoc_name,
            selection
                .as_ref()
                .map(|selection| (selection.trait_impl.impl_ref, &selection.subst)),
            selection_table,
        )?;
        let mut projection = match projection {
            ChalkOutcome::Proven(projection) => projection,
            ChalkOutcome::Ambiguous(Some(projection)) => projection,
            ChalkOutcome::Ambiguous(None)
            | ChalkOutcome::NoSolution
            | ChalkOutcome::Unsupported
            | ChalkOutcome::Exhausted => return Ok(None),
        };
        if matches!(projection.ty, Ty::Unknown) {
            return Ok(None);
        }
        if let Some(selection) = selection {
            projection.applicability = selection.applicability.and(projection.applicability);
        }
        Ok(Some(projection))
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

                let Some(data) = self
                    .context
                    .item_paths()
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
                let normalized =
                    self.normalize_assoc_type_once(&goal, data.name.as_str(), table)?;
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
}
