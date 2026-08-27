//! Associated-type and canonical type normalization for `TraitSelectionQuery`.
//!
//! A selected nominal impl can supply its associated value directly from the semantic declaration.
//! Chalk handles the remaining cases, including environment evidence such as an opaque type's
//! declared bounds and goals without one exact impl. Recursive normalization re-enters this same
//! bounded boundary so nested aliases use the same evidence and inference table.

use rg_def_map::DefMapSource;
use rg_ir_model::{
    GenericDefRef, ImplRef, ItemOwner, TraitApplicability, TraitDefRef, TypeAliasRef,
};
use rg_semantic_ir::ItemStoreSource;
use rg_std::ExpectedUnique;

use super::matcher::TraitSelfHead;
use super::session::{TraitWorkKind, TraitWorkLimit};
use super::{ChalkOutcome, TraitGoal, TraitSelection, TraitSelectionQuery};
use crate::inference::InferenceTable;
use crate::{
    AdtTy, AliasTy, ClosureTy, FnDefTy, GenericArg, GenericArgs, OpaqueTy, ProjectionTy,
    Substitution, TraitApplication, Ty,
};

// Projection results can keep producing a different, larger projection without repeating an
// exact semantic cycle. Bound that expansion before it can consume the thread stack.
pub(super) const NORMALIZATION_DEPTH_LIMIT: usize = 64;

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

/// Native impls already being proved while recursively normalizing associated projections.
///
/// Candidate predicates often mention an associated type supplied by a different predicate-free
/// impl. That declaration is cheap native evidence and should not force the entire predicate into
/// Chalk. The path also tells normalization to preserve solver-shaped projections for the outer
/// combined goal instead of solving each one independently.
#[derive(Clone, Copy)]
pub(super) struct CandidateEvidence<'path> {
    active_impls: &'path [ImplRef],
    native_declarations_only: bool,
}

impl CandidateEvidence<'static> {
    pub(super) const ROOT: Self = Self {
        active_impls: &[],
        native_declarations_only: false,
    };
}

impl<'path> CandidateEvidence<'path> {
    pub(super) fn within(active_impls: &'path [ImplRef]) -> Self {
        Self {
            active_impls,
            native_declarations_only: true,
        }
    }

    pub(super) fn active_impls(self) -> &'path [ImplRef] {
        self.active_impls
    }

    pub(super) fn native_only(self) -> Self {
        Self {
            native_declarations_only: true,
            ..self
        }
    }

    pub(super) fn allows_solver_fallback(self) -> bool {
        !self.native_declarations_only
    }
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
    /// Normalize a named associated type through exact impl evidence or Chalk.
    ///
    /// A unique native impl lets the adapter instantiate its matching associated declaration
    /// directly. Opaque bounds and goals without one exact impl enter the bounded solver forest.
    /// If neither path can model or decode the projection, the query returns no semantic fact.
    pub fn normalize_assoc_type(
        &self,
        goal: &TraitGoal,
        assoc_name: &str,
        table: &InferenceTable,
    ) -> Result<Option<AssocProjectionResult>, I::Error> {
        let Some(associated_ty) = self.associated_type_named(goal.trait_ref(), assoc_name)? else {
            return Ok(None);
        };
        let Some(mut projection) =
            self.normalize_assoc_type_once(goal, associated_ty, table, CandidateEvidence::ROOT)?
        else {
            return Ok(None);
        };
        let mut table = projection.table;
        projection.ty = self.normalize_ty_with_table(
            &projection.ty,
            &mut table,
            &mut Vec::new(),
            CandidateEvidence::ROOT,
            0,
        )?;
        projection.table = table;
        Ok(Some(projection))
    }

    /// Project one associated value without recursively normalizing aliases inside that value.
    fn normalize_assoc_type_once(
        &self,
        goal: &TraitGoal,
        associated_ty: TypeAliasRef,
        table: &InferenceTable,
        candidate_evidence: CandidateEvidence<'_>,
    ) -> Result<Option<AssocProjectionResult>, I::Error> {
        if !candidate_evidence.allows_solver_fallback() {
            let self_ty = table.resolve_root_var(goal.self_ty());
            let is_stable_headed_goal = goal.is_cache_stable()
                && !goal.application.args.iter().any(GenericArg::has_unknown)
                && TraitSelfHead::from_ty(&self_ty).is_some();
            if !is_stable_headed_goal {
                crate::profile::metric::NATIVE_CANDIDATE_UNSTABLE_DECLINES.inc();
                return Ok(None);
            }
        }

        let (selection, fully_evaluated) =
            self.probe_with_completeness_avoiding(goal, table, candidate_evidence)?;
        if !candidate_evidence.allows_solver_fallback() && !fully_evaluated {
            // A predicate-bearing or recursive competing candidate can change the associated
            // value. Keep the equality for Chalk unless native discovery classified the complete
            // matching set.
            return Ok(None);
        }
        let selection = match selection {
            ExpectedUnique::One(selection) => Some(selection),
            // Native matching indexes every ordinary impl by the concrete ADT head. Once all
            // matching candidates were classified, Chalk has no additional source of evidence
            // for that nominal type. Opaque, generic, and callable types still fall through
            // because their bounds or built-in clauses can prove a goal with no ordinary impl
            // identity.
            ExpectedUnique::Empty
                if fully_evaluated
                    && matches!(table.resolve_root_var(goal.self_ty()), Ty::Adt(_)) =>
            {
                return Ok(None);
            }
            ExpectedUnique::Empty | ExpectedUnique::Ambiguous => None,
        };

        let selection_table = selection
            .as_ref()
            .map(|selection| &selection.table)
            .unwrap_or(table);

        // An exact native candidate already carries everything needed to instantiate a plain
        // associated declaration. Keep that semantic operation on this side of the Chalk adapter;
        // only defaults, opaque evidence, GATs, and other solver-shaped cases cross the boundary.
        if let Some(selection) = &selection
            && let Some(projection) = self.project_selected_impl(goal, associated_ty, selection)?
        {
            crate::profile::metric::NATIVE_ASSOC_PROJECTIONS.inc();
            return Ok(Some(projection));
        }

        // Native-only normalization is performed while preparing a larger predicate conjunction.
        // Preserve anything that is not a direct declaration so the caller submits one combined
        // Chalk goal instead of materializing a transitive program for each equality separately.
        if !candidate_evidence.allows_solver_fallback() {
            return Ok(None);
        }

        let projection = self.context.trait_selection().normalize_assoc_type(
            self.context.item_paths(),
            self.context.crate_items(),
            self.context.item_lookup(),
            goal,
            associated_ty,
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

    /// Normalize one already-resolved projection identity through the same candidate path as a
    /// type-tree projection.
    ///
    /// Alias-equality predicates use this entry point before native proof. A definite selected
    /// value can discharge the equality without asking Chalk to rediscover the same impl.
    pub(super) fn normalize_projection_once(
        &self,
        alias: &ProjectionTy,
        table: &InferenceTable,
        candidate_evidence: CandidateEvidence<'_>,
    ) -> Result<Option<AssocProjectionResult>, I::Error> {
        let Some(data) = self
            .context
            .item_paths()
            .items()
            .type_alias_data(alias.associated_ty)?
        else {
            return Ok(None);
        };
        let ItemOwner::Trait(trait_id) = data.owner else {
            return Ok(None);
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
        self.normalize_assoc_type_once(&goal, alias.associated_ty, table, candidate_evidence)
    }

    /// Instantiate a plain associated value from an already-proved concrete impl.
    ///
    /// Once the array `IntoIterator` impl is selected, this turns
    /// `<[User; 3] as IntoIterator>::IntoIter` into `array::IntoIter<User, 3>`. A bound such as
    /// `type IntoIter: Iterator` constrains that exact value but does not hide it; generic
    /// associated types still need their own binder-aware path.
    fn project_selected_impl(
        &self,
        goal: &TraitGoal,
        associated_ty: TypeAliasRef,
        selection: &TraitSelection,
    ) -> Result<Option<AssocProjectionResult>, I::Error> {
        // A blanket impl selected for an opaque receiver may derive its value from that opaque's
        // declared equality. Leave that environment evidence to Chalk. Concrete structural heads
        // are as stable as nominal ones here because native selection already proved the exact impl.
        let self_ty = selection.table.resolve_root_var(goal.self_ty());
        if TraitSelfHead::from_ty(&self_ty).is_none() {
            return Ok(None);
        }

        // Generic associated types need their own argument binders before their value can be
        // instantiated here. Plain associated-type bounds do not change that value: for
        // `type IntoIter: Iterator`, the bound constrains every valid implementation, while an
        // exact selected impl still supplies `IntoIter` directly.
        let can_project_directly =
            |data: &rg_semantic_ir::TypeAliasData| data.signature.generics().is_none();
        let Some(trait_alias_data) = self
            .context
            .item_paths()
            .items()
            .type_alias_data(associated_ty)?
        else {
            return Ok(None);
        };
        if !can_project_directly(trait_alias_data) {
            return Ok(None);
        }
        let Some(alias) = self
            .context
            .item_paths()
            .items()
            .impl_associated_type_by_name(
                selection.trait_impl.impl_ref,
                trait_alias_data.name.as_str(),
            )?
        else {
            return Ok(None);
        };
        let Some(alias_data) = self.context.item_paths().items().type_alias_data(alias)? else {
            return Ok(None);
        };
        if !can_project_directly(alias_data) {
            return Ok(None);
        }
        let Some(selected_value) = self
            .context
            .trait_selection()
            .type_alias_ty_with(self.context.item_paths(), alias)?
        else {
            return Ok(None);
        };

        let generics = self
            .context
            .item_paths()
            .generics()
            .generics(GenericDefRef::Impl(selection.trait_impl.impl_ref))?;
        let args = selection.subst.as_substitution().args_for(&generics);
        let complete_subst = Substitution::from_args(&generics, &args);
        Ok(Some(AssocProjectionResult {
            ty: selection
                .table
                .canonicalize(&complete_subst.apply(&selected_value)),
            applicability: selection.applicability,
            table: selection.table.clone(),
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
        self.normalize_ty_with_candidate_evidence(ty, table, CandidateEvidence::ROOT)
    }

    pub(super) fn normalize_ty_with_candidate_evidence(
        &self,
        ty: &Ty,
        table: &InferenceTable,
        candidate_evidence: CandidateEvidence<'_>,
    ) -> Result<(Ty, InferenceTable), I::Error> {
        let mut table = table.clone();
        let ty =
            self.normalize_ty_with_table(ty, &mut table, &mut Vec::new(), candidate_evidence, 0)?;
        Ok((ty, table))
    }

    fn normalize_ty_with_table(
        &self,
        ty: &Ty,
        table: &mut InferenceTable,
        active: &mut Vec<ProjectionTy>,
        candidate_evidence: CandidateEvidence<'_>,
        depth: usize,
    ) -> Result<Ty, I::Error> {
        let session = self.context.trait_selection();
        if depth >= NORMALIZATION_DEPTH_LIMIT {
            session.report_limit(
                TraitWorkLimit::NormalizationDepth,
                Some(NORMALIZATION_DEPTH_LIMIT),
            );
            return Ok(ty.clone());
        }
        if !session.consume_work(TraitWorkKind::NormalizationStep, 1) {
            return Ok(ty.clone());
        }
        let child_depth = depth + 1;

        Ok(match ty {
            Ty::Tuple(fields) => Ty::tuple(
                fields
                    .iter()
                    .map(|field| {
                        self.normalize_ty_with_table(
                            field,
                            table,
                            active,
                            candidate_evidence,
                            child_depth,
                        )
                    })
                    .collect::<Result<_, _>>()?,
            ),
            Ty::Array { inner, len } => Ty::Array {
                inner: Box::new(self.normalize_ty_with_table(
                    inner,
                    table,
                    active,
                    candidate_evidence,
                    child_depth,
                )?),
                len: *len,
            },
            Ty::Slice(inner) => Ty::slice(self.normalize_ty_with_table(
                inner,
                table,
                active,
                candidate_evidence,
                child_depth,
            )?),
            Ty::Reference {
                lifetime,
                mutability,
                inner,
            } => Ty::reference_with_lifetime(
                *lifetime,
                *mutability,
                self.normalize_ty_with_table(
                    inner,
                    table,
                    active,
                    candidate_evidence,
                    child_depth,
                )?,
            ),
            Ty::RawPointer { mutability, inner } => Ty::raw_pointer(
                *mutability,
                self.normalize_ty_with_table(
                    inner,
                    table,
                    active,
                    candidate_evidence,
                    child_depth,
                )?,
            ),
            Ty::FnPointer { params, ret } => Ty::fn_pointer(
                params
                    .iter()
                    .map(|param| {
                        self.normalize_ty_with_table(
                            param,
                            table,
                            active,
                            candidate_evidence,
                            child_depth,
                        )
                    })
                    .collect::<Result<_, _>>()?,
                self.normalize_ty_with_table(ret, table, active, candidate_evidence, child_depth)?,
            ),
            Ty::Adt(ty) => Ty::Adt(AdtTy {
                def: ty.def,
                args: self.normalize_args(
                    &ty.args,
                    table,
                    active,
                    candidate_evidence,
                    child_depth,
                )?,
            }),
            Ty::FnDef(function) => Ty::FnDef(FnDefTy {
                def: function.def,
                args: self.normalize_args(
                    &function.args,
                    table,
                    active,
                    candidate_evidence,
                    child_depth,
                )?,
            }),
            Ty::Closure(closure) => Ty::Closure(ClosureTy {
                id: closure.id,
                params: closure
                    .params
                    .iter()
                    .map(|param| {
                        self.normalize_ty_with_table(
                            param,
                            table,
                            active,
                            candidate_evidence,
                            child_depth,
                        )
                    })
                    .collect::<Result<_, _>>()?,
                ret: Box::new(self.normalize_ty_with_table(
                    &closure.ret,
                    table,
                    active,
                    candidate_evidence,
                    child_depth,
                )?),
            }),
            Ty::Alias(AliasTy::Opaque(opaque)) => Ty::Alias(AliasTy::Opaque(OpaqueTy {
                opaque: opaque.opaque,
                args: self.normalize_args(
                    &opaque.args,
                    table,
                    active,
                    candidate_evidence,
                    child_depth,
                )?,
            })),
            Ty::Alias(AliasTy::Projection(alias)) => {
                let alias = ProjectionTy {
                    associated_ty: alias.associated_ty,
                    args: self.normalize_args(
                        &alias.args,
                        table,
                        active,
                        candidate_evidence,
                        child_depth,
                    )?,
                };
                // Solver retries can reproduce one projection with freshly allocated inference
                // slots. Their numeric IDs differ, but they still describe the same obligation.
                if active
                    .iter()
                    .any(|active_alias| active_alias.equivalent_modulo_inference_ids(&alias))
                {
                    return Ok(Ty::Alias(AliasTy::Projection(alias)));
                }

                // Associated values can form cycles across multiple aliases. Stop at the first
                // repeated semantic projection and keep that alias visible to the caller.
                active.push(alias.clone());
                let normalized =
                    self.normalize_projection_once(&alias, table, candidate_evidence)?;
                let ty = if let Some(normalized) = normalized {
                    let (ty, _applicability, normalized_table) = normalized.into_parts();
                    *table = normalized_table;
                    self.normalize_ty_with_table(
                        &ty,
                        table,
                        active,
                        candidate_evidence,
                        child_depth,
                    )?
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
            | Ty::Unknown
            | Ty::InferVar { .. } => ty.clone(),
        })
    }

    /// Resolve the convenient named API to one declaration before entering the solver boundary.
    fn associated_type_named(
        &self,
        trait_ref: TraitDefRef,
        name: &str,
    ) -> Result<Option<TypeAliasRef>, I::Error> {
        self.context
            .crate_items()
            .items()
            .declared_associated_type_by_name(trait_ref, name)
    }

    fn normalize_args(
        &self,
        args: &GenericArgs,
        table: &mut InferenceTable,
        active: &mut Vec<ProjectionTy>,
        candidate_evidence: CandidateEvidence<'_>,
        depth: usize,
    ) -> Result<GenericArgs, I::Error> {
        args.iter()
            .map(|arg| {
                Ok(match arg {
                    GenericArg::Type(ty) => GenericArg::Type(Box::new(
                        self.normalize_ty_with_table(ty, table, active, candidate_evidence, depth)?,
                    )),
                    GenericArg::Lifetime(_) | GenericArg::Const(_) => arg.clone(),
                })
            })
            .collect()
    }
}
