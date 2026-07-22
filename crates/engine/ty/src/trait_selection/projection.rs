//! Associated-type and canonical type normalization for `TraitSelectionQuery`.
//!
//! A selected nominal impl can supply its associated value directly from the semantic declaration.
//! Chalk handles the remaining cases, including environment evidence such as an opaque type's
//! declared bounds and goals without one exact impl. Recursive normalization re-enters this same
//! bounded boundary so nested aliases use the same evidence and inference table.

use rg_def_map::DefMapSource;
use rg_ir_model::{GenericDefRef, ItemOwner, TraitApplicability, TraitDefRef, TypeAliasRef};
use rg_semantic_ir::ItemStoreSource;
use rg_std::ExpectedUnique;

use super::{ChalkOutcome, TraitGoal, TraitSelection, TraitSelectionQuery};
use crate::inference::InferenceTable;
use crate::{
    AdtTy, AliasTy, ClosureTy, FnDefTy, GenericArg, GenericArgs, OpaqueTy, ProjectionTy,
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

/// Whether recursive normalization may use native candidate selection as extra evidence.
///
/// An outer query starts in `Probe` mode. Once candidate predicates are being proved, recursively
/// probing the same candidate would re-enter native selection; `SolverOnly` leaves that cycle to
/// Chalk's own forest instead.
#[derive(Clone, Copy)]
pub(super) enum CandidateEvidence {
    /// Use native selection to instantiate an already-proved impl value before solver search.
    Probe,
    /// Stay inside Chalk while normalizing a predicate that native selection is proving.
    SolverOnly,
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
            self.normalize_assoc_type_once(goal, associated_ty, table, CandidateEvidence::Probe)?
        else {
            return Ok(None);
        };
        let mut table = projection.table;
        projection.ty = self.normalize_ty_with_table(
            &projection.ty,
            &mut table,
            &mut Vec::new(),
            CandidateEvidence::Probe,
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
        candidate_evidence: CandidateEvidence,
    ) -> Result<Option<AssocProjectionResult>, I::Error> {
        let selection = match candidate_evidence {
            CandidateEvidence::Probe => {
                let (selection, fully_evaluated) = self.probe_with_completeness(goal, table)?;
                match selection {
                    ExpectedUnique::One(selection) => Some(selection),
                    // Native matching indexes every ordinary impl by the concrete ADT head. Once
                    // all matching candidates were classified, Chalk has no additional source of
                    // evidence for that nominal type. Opaque, generic, and callable types still
                    // fall through because their bounds or built-in clauses can prove a goal with
                    // no ordinary impl identity.
                    ExpectedUnique::Empty
                        if fully_evaluated
                            && matches!(table.resolve_root_var(goal.self_ty()), Ty::Adt(_)) =>
                    {
                        return Ok(None);
                    }
                    ExpectedUnique::Empty | ExpectedUnique::Ambiguous => None,
                }
            }
            CandidateEvidence::SolverOnly => None,
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
            return Ok(Some(projection));
        }

        let projection = self.context.trait_selection().normalize_assoc_type(
            self.context.item_paths(),
            self.context.crate_items(),
            self.context.lookup_index(),
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

    /// Instantiate a plain associated value from an already-proved nominal impl.
    fn project_selected_impl(
        &self,
        goal: &TraitGoal,
        associated_ty: TypeAliasRef,
        selection: &TraitSelection,
    ) -> Result<Option<AssocProjectionResult>, I::Error> {
        // A blanket impl selected for an opaque receiver may derive its value from that opaque's
        // declared equality. Leave that environment evidence to Chalk; this shortcut exists for
        // the indexed nominal receiver that native selection proved directly.
        if !matches!(selection.table.resolve_root_var(goal.self_ty()), Ty::Adt(_)) {
            return Ok(None);
        }

        // Generic associated types and required bounds need binders or additional predicates.
        // Relaxed bounds such as `?Sized` add no requirement in rust-glancer's semantic model.
        let can_project_directly = |data: &rg_semantic_ir::TypeAliasData| {
            data.signature.generics().is_none()
                && data
                    .signature
                    .bounds()
                    .iter()
                    .all(rg_item_tree::TypeBound::is_relaxed_trait)
        };
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
        let Some(selected_value) =
            SemanticSignatureQuery::type_alias_ty_from(self.context.item_paths(), alias)?
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
        self.normalize_ty_with_candidate_evidence(ty, table, CandidateEvidence::Probe)
    }

    pub(super) fn normalize_ty_with_candidate_evidence(
        &self,
        ty: &Ty,
        table: &InferenceTable,
        candidate_evidence: CandidateEvidence,
    ) -> Result<(Ty, InferenceTable), I::Error> {
        let mut table = table.clone();
        let ty =
            self.normalize_ty_with_table(ty, &mut table, &mut Vec::new(), candidate_evidence)?;
        Ok((ty, table))
    }

    fn normalize_ty_with_table(
        &self,
        ty: &Ty,
        table: &mut InferenceTable,
        active: &mut Vec<ProjectionTy>,
        candidate_evidence: CandidateEvidence,
    ) -> Result<Ty, I::Error> {
        Ok(match ty {
            Ty::Tuple(fields) => Ty::tuple(
                fields
                    .iter()
                    .map(|field| {
                        self.normalize_ty_with_table(field, table, active, candidate_evidence)
                    })
                    .collect::<Result<_, _>>()?,
            ),
            Ty::Array { inner, len } => Ty::Array {
                inner: Box::new(self.normalize_ty_with_table(
                    inner,
                    table,
                    active,
                    candidate_evidence,
                )?),
                len: *len,
            },
            Ty::Slice(inner) => {
                Ty::slice(self.normalize_ty_with_table(inner, table, active, candidate_evidence)?)
            }
            Ty::Reference {
                lifetime,
                mutability,
                inner,
            } => Ty::reference_with_lifetime(
                *lifetime,
                *mutability,
                self.normalize_ty_with_table(inner, table, active, candidate_evidence)?,
            ),
            Ty::RawPointer { mutability, inner } => Ty::raw_pointer(
                *mutability,
                self.normalize_ty_with_table(inner, table, active, candidate_evidence)?,
            ),
            Ty::FnPointer { params, ret } => Ty::fn_pointer(
                params
                    .iter()
                    .map(|param| {
                        self.normalize_ty_with_table(param, table, active, candidate_evidence)
                    })
                    .collect::<Result<_, _>>()?,
                self.normalize_ty_with_table(ret, table, active, candidate_evidence)?,
            ),
            Ty::Adt(ty) => Ty::Adt(AdtTy {
                def: ty.def,
                args: self.normalize_args(&ty.args, table, active, candidate_evidence)?,
            }),
            Ty::FnDef(function) => Ty::FnDef(FnDefTy {
                def: function.def,
                args: self.normalize_args(&function.args, table, active, candidate_evidence)?,
            }),
            Ty::Closure(closure) => Ty::Closure(ClosureTy {
                id: closure.id,
                params: closure
                    .params
                    .iter()
                    .map(|param| {
                        self.normalize_ty_with_table(param, table, active, candidate_evidence)
                    })
                    .collect::<Result<_, _>>()?,
                ret: Box::new(self.normalize_ty_with_table(
                    &closure.ret,
                    table,
                    active,
                    candidate_evidence,
                )?),
            }),
            Ty::Alias(AliasTy::Opaque(opaque)) => Ty::Alias(AliasTy::Opaque(OpaqueTy {
                opaque: opaque.opaque,
                args: self.normalize_args(&opaque.args, table, active, candidate_evidence)?,
            })),
            Ty::Alias(AliasTy::Projection(alias)) => {
                let alias = ProjectionTy {
                    associated_ty: alias.associated_ty,
                    args: self.normalize_args(&alias.args, table, active, candidate_evidence)?,
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
                let normalized = self.normalize_assoc_type_once(
                    &goal,
                    alias.associated_ty,
                    table,
                    candidate_evidence,
                )?;
                let ty = if let Some(normalized) = normalized {
                    let (ty, _applicability, normalized_table) = normalized.into_parts();
                    *table = normalized_table;
                    self.normalize_ty_with_table(&ty, table, active, candidate_evidence)?
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
        candidate_evidence: CandidateEvidence,
    ) -> Result<GenericArgs, I::Error> {
        args.iter()
            .map(|arg| {
                Ok(match arg {
                    GenericArg::Type(ty) => GenericArg::Type(Box::new(
                        self.normalize_ty_with_table(ty, table, active, candidate_evidence)?,
                    )),
                    GenericArg::Lifetime(_) | GenericArg::Const(_) => arg.clone(),
                })
            })
            .collect()
    }
}
