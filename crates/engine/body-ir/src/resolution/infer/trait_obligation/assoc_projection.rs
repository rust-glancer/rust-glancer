//! Body-aware normalization of canonical associated-type projections.
//!
//! Pure trait selection can project impls whose header and Chalk predicates determine every
//! parameter. A closure is different: its `Fn*::Output` is evidence owned by the active body, so
//! an impl-only parameter such as `R` below cannot be solved without the body's inference facts.
//!
//! ```text
//! impl<F, R> Produces for Adapter<F>
//! where
//!     F: FnOnce() -> R,
//! {
//!     type Output = R;
//! }
//! ```
//!
//! This module keeps that body interaction above the semantic boundary. It selects the canonical
//! `ImplHeader`, instantiates its owner-scoped params, evaluates its already-lowered clauses, and
//! finally substitutes the canonical associated value. Declaration `TypeRef` syntax is never
//! interpreted again here.

use rg_def_map::DefMapSource;
use rg_ir_model::{AssocItemId, ItemOwner, TraitDefRef, TypeAliasRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::{
    AdtTy, AliasTy, FnDefTy, GenericArg, GenericArgs, OpaqueTy, ProjectionTy, TraitApplication,
    TraitGoal, Ty,
};

use super::super::BodyInferenceCtx;
use super::{BodyTraitGoalOutcome, BodyTraitObligationSolver};

enum BodyAssocProjection {
    Projected(Ty),
    Rejected,
    Unavailable,
}

impl<'query, D, I> BodyTraitObligationSolver<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    /// Normalize every associated projection in `ty`, admitting body-local callable evidence.
    pub(crate) fn normalize_ty(
        &self,
        inference: &mut BodyInferenceCtx,
        ty: &Ty,
    ) -> Result<Ty, PackageStoreError> {
        self.normalize_ty_inner(inference, ty, &mut Vec::new())
    }

    fn normalize_ty_inner(
        &self,
        inference: &mut BodyInferenceCtx,
        ty: &Ty,
        active: &mut Vec<ProjectionTy>,
    ) -> Result<Ty, PackageStoreError> {
        let ty = inference.table.canonicalize(ty);
        Ok(match ty {
            Ty::Tuple(fields) => Ty::tuple(
                fields
                    .iter()
                    .map(|field| self.normalize_ty_inner(inference, field, active))
                    .collect::<Result<_, _>>()?,
            ),
            Ty::Array { inner, len } => {
                Ty::array(self.normalize_ty_inner(inference, &inner, active)?, len)
            }
            Ty::Slice(inner) => Ty::slice(self.normalize_ty_inner(inference, &inner, active)?),
            Ty::Reference {
                lifetime,
                mutability,
                inner,
            } => Ty::reference_with_lifetime(
                lifetime,
                mutability,
                self.normalize_ty_inner(inference, &inner, active)?,
            ),
            Ty::RawPointer { mutability, inner } => Ty::raw_pointer(
                mutability,
                self.normalize_ty_inner(inference, &inner, active)?,
            ),
            Ty::FnPointer { params, ret } => Ty::fn_pointer(
                params
                    .iter()
                    .map(|param| self.normalize_ty_inner(inference, param, active))
                    .collect::<Result<_, _>>()?,
                self.normalize_ty_inner(inference, &ret, active)?,
            ),
            Ty::Adt(ty) => Ty::Adt(AdtTy {
                def: ty.def,
                args: self.normalize_args(inference, &ty.args, active)?,
            }),
            Ty::FnDef(function) => Ty::FnDef(FnDefTy {
                def: function.def,
                args: self.normalize_args(inference, &function.args, active)?,
            }),
            Ty::Alias(AliasTy::Opaque(opaque)) => Ty::Alias(AliasTy::Opaque(OpaqueTy {
                opaque: opaque.opaque,
                args: self.normalize_args(inference, &opaque.args, active)?,
            })),
            Ty::Alias(AliasTy::Projection(alias)) => {
                let alias = ProjectionTy {
                    associated_ty: alias.associated_ty,
                    args: self.normalize_args(inference, &alias.args, active)?,
                };
                if active.iter().any(|active| {
                    active.associated_ty == alias.associated_ty
                        && active.args.equivalent_modulo_inference_ids(&alias.args)
                }) {
                    return Ok(Ty::Alias(AliasTy::Projection(alias)));
                }

                // Let the shared solver own ordinary projections. The body path is only a second
                // chance for a projection that remained unchanged because it needs local closure
                // evidence.
                let alias_ty = Ty::Alias(AliasTy::Projection(alias.clone()));
                let (shared_ty, table) = self
                    .context
                    .trait_selection_with_cache(inference.trait_selection_cache())
                    .normalize_ty(&alias_ty, &inference.table)?;
                inference.table = table;
                if shared_ty != alias_ty {
                    return self.normalize_ty_inner(inference, &shared_ty, active);
                }

                let Some(data) = self
                    .context
                    .item_query()
                    .type_alias_data(alias.associated_ty)?
                else {
                    return Ok(alias_ty);
                };
                let ItemOwner::Trait(trait_id) = data.owner else {
                    return Ok(alias_ty);
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

                active.push(alias.clone());
                let projected =
                    self.project_selected_impl_assoc(inference, &goal, data.name.as_str());
                let normalized = match projected? {
                    BodyAssocProjection::Projected(projected) => {
                        self.normalize_ty_inner(inference, &projected, active)?
                    }
                    BodyAssocProjection::Rejected => Ty::Unknown,
                    BodyAssocProjection::Unavailable => alias_ty,
                };
                active.pop();
                normalized
            }
            Ty::Unit
            | Ty::Never
            | Ty::Primitive(_)
            | Ty::Param(_)
            | Ty::Closure(_)
            | Ty::Unknown
            | Ty::InferVar { .. } => ty,
        })
    }

    fn normalize_args(
        &self,
        inference: &mut BodyInferenceCtx,
        args: &GenericArgs,
        active: &mut Vec<ProjectionTy>,
    ) -> Result<GenericArgs, PackageStoreError> {
        args.iter()
            .map(|arg| {
                Ok(match arg {
                    GenericArg::Type(ty) => {
                        GenericArg::Type(Box::new(self.normalize_ty_inner(inference, ty, active)?))
                    }
                    GenericArg::Lifetime(_) | GenericArg::Const(_) => arg.clone(),
                })
            })
            .collect()
    }

    /// Select one impl without asking Chalk to invent body facts, then solve its canonical clauses
    /// against a trial copy of the active body inference state.
    fn project_selected_impl_assoc(
        &self,
        inference: &mut BodyInferenceCtx,
        goal: &TraitGoal,
        assoc_name: &str,
    ) -> Result<BodyAssocProjection, PackageStoreError> {
        let Some(selected) = self.select_impl_for_body(inference, goal)? else {
            return Ok(BodyAssocProjection::Unavailable);
        };
        let impl_ref = selected.selection.trait_impl.impl_ref;

        let mut trial = inference.clone();
        trial.table = selected.selection.table;
        let goals = Self::trait_goals_from_clauses(
            &selected.header.clauses,
            selected.selection.subst.as_substitution(),
        );
        match self.evaluate_trait_goals(&mut trial, goals)? {
            BodyTraitGoalOutcome::Solved => {}
            // This evaluator is intentionally bounded and may be waiting for body-local facts.
            // Preserve the projection so a later fixed-point pass can retry it.
            BodyTraitGoalOutcome::Deferred => return Ok(BodyAssocProjection::Unavailable),
            BodyTraitGoalOutcome::Rejected => return Ok(BodyAssocProjection::Rejected),
        }

        let Some(impl_data) = self.context.item_query().impl_data(impl_ref)? else {
            return Ok(BodyAssocProjection::Unavailable);
        };
        for item in &impl_data.items {
            let AssocItemId::TypeAlias(id) = item else {
                continue;
            };
            let alias = TypeAliasRef {
                origin: impl_ref.origin,
                id: *id,
            };
            let Some(data) = self.context.item_query().type_alias_data(alias)? else {
                continue;
            };
            if data.name.as_str() != assoc_name {
                continue;
            }
            let Some(ty) = self.context.signatures().type_alias_ty(alias)? else {
                return Ok(BodyAssocProjection::Unavailable);
            };
            let ty = selected.selection.subst.as_substitution().apply(&ty);
            let ty = trial.table.canonicalize(&ty);
            *inference = trial;
            return Ok(BodyAssocProjection::Projected(ty));
        }

        Ok(BodyAssocProjection::Unavailable)
    }
}
