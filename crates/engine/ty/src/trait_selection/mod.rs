//! Bounded trait-impl selection shared by inference and editor queries.
//!
//! Native matching discovers canonical impl headers that may fit a resolved trait goal. A small
//! bounded native proof handles concrete impl chains and compiler-known closure facts; Chalk owns
//! the remaining predicates and associated-type equalities. Keeping discovery, native proof, and
//! solver fallback as different types prevents exploratory editor candidates from being mistaken
//! for established semantic facts.
//!
//! Canonical crate declarations have a wider reuse boundary than solver state. Their lowered types
//! can be shared by sessions over the same semantic snapshot, while visible impl indexes, Chalk
//! forests, body declarations, and inference answers remain owned by the use-site session that
//! produced them.

mod candidate;
mod chalk;
mod declaration_cache;
mod matcher;
mod native_proof;
mod projection;
mod session;

use rg_def_map::DefMapSource;
use rg_ir_model::{GenericDefRef, ImplRef, TraitApplicability, TraitImplRef, TypeAliasRef};
use rg_semantic_ir::ItemStoreSource;
use rg_std::ExpectedUnique;

use self::candidate::TraitCandidate;
use self::chalk::ChalkOutcome;
pub use self::declaration_cache::TraitSelectionDeclarationCache;
use self::native_proof::NativeProofQuery;
pub use self::projection::AssocProjectionResult;
use self::projection::CandidateEvidence;
pub use self::session::TraitSelectionSession;
use crate::inference::{InferenceSubstitution, InferenceTable};
use crate::{
    AssocTypeBinding, Clause, GenericArg, GenericArgs, Substitution, TraitApplication,
    TraitRefLowering, Ty, TyContext,
};

/// A canonical trait application plus any associated-type equality constraints.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TraitGoal {
    pub application: TraitApplication,
    pub associated_types: Vec<AssocTypeBinding>,
}

/// One `Trait<Assoc = Ty>` equality constraint carried by a trait goal.
pub(crate) struct AssocTypeConstraint<'a> {
    pub(crate) associated_ty: TypeAliasRef,
    pub(crate) ty: &'a Ty,
}

impl TraitGoal {
    /// Build a goal from positional arguments that do not include `Self`.
    pub fn new(
        self_ty: Ty,
        trait_ref: rg_ir_model::TraitDefRef,
        args: impl Into<GenericArgs>,
    ) -> Self {
        let args = args.into();
        let mut full_args = Vec::with_capacity(1 + args.len());
        full_args.push(GenericArg::Type(Box::new(self_ty)));
        full_args.extend(args.into_vec());
        Self {
            application: TraitApplication {
                def: trait_ref,
                args: full_args.into(),
            },
            associated_types: Vec::new(),
        }
    }

    pub fn from_lowering(lowering: TraitRefLowering) -> Self {
        Self {
            application: lowering.application,
            associated_types: lowering.associated_types,
        }
    }

    pub fn self_ty(&self) -> &Ty {
        self.application
            .self_ty()
            .expect("trait applications always contain the Self argument")
    }

    pub fn trait_ref(&self) -> rg_ir_model::TraitDefRef {
        self.application.def
    }

    /// Iterate trait input args without associated-type equality constraints.
    ///
    /// Rust syntax puts both shapes inside the same angle brackets:
    ///
    /// ```text
    /// Iterator<Item = User>
    /// Indexed<Key, Item = User>
    /// ```
    ///
    /// Only the positional inputs belong in the trait substitution that Chalk sees as
    /// `Implemented(Self: Trait<...>)`. Associated equality args are separate projection
    /// constraints, such as `<Self as Iterator>::Item = User`.
    pub fn iter_positional_args(&self) -> impl Iterator<Item = &GenericArg> {
        self.application.args.iter().skip(1)
    }

    pub(crate) fn without_assoc_type_constraints(&self) -> Self {
        Self {
            application: self.application.clone(),
            associated_types: Vec::new(),
        }
    }

    pub(crate) fn has_assoc_type_constraints(&self) -> bool {
        !self.associated_types.is_empty()
    }

    pub(crate) fn assoc_type_constraints(&self) -> impl Iterator<Item = AssocTypeConstraint<'_>> {
        self.associated_types
            .iter()
            .map(|binding| AssocTypeConstraint {
                associated_ty: binding.associated_ty,
                ty: &binding.ty,
            })
    }

    /// Return whether this goal is independent of one body's live inference state.
    ///
    /// Semantic unknowns and projections are stable values: a later, more precise query produces a
    /// different goal. Inference variables and closure identities instead belong to the caller's
    /// table/body and must be classified again there.
    fn is_cache_stable(&self) -> bool {
        self.application
            .args
            .iter()
            .all(|arg| !arg.has_var() && !arg.has_closure())
            && self
                .associated_types
                .iter()
                .all(|binding| !binding.ty.has_var() && !binding.ty.has_closure())
    }
}

/// One visible trait impl after the bounded proof pipeline classified its remaining conditions.
///
/// `Yes` means its predicates and associated-type constraints were proved. `Maybe` preserves a
/// plausible editor candidate when matching or proof is ambiguous, or when the bounded adapter
/// cannot finish; callers must not mistake it for established semantic evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitSelection {
    pub trait_impl: TraitImplRef,
    pub subst: InferenceSubstitution,
    pub applicability: TraitApplicability,
    /// Trial table after applying this candidate's direct equality evidence.
    ///
    /// Candidate evaluation never mutates the caller's table. Once a caller chooses this
    /// selection, it can explicitly adopt the table together with the selected impl.
    pub table: InferenceTable,
}

/// Semantic result of proving a related set of trait predicates.
///
/// `Proven` carries a trial inference table with every equality learned while proving. Ambiguity can
/// carry partial guidance too, but callers must not treat that guidance as a completed proof.
/// `Unavailable` means the bounded adapter could not model or finish the query; it is deliberately
/// separate from Rust-level `NoSolution`.
#[derive(Debug, Clone)]
pub enum TraitProof {
    Proven(InferenceTable),
    Ambiguous(Option<InferenceTable>),
    NoSolution,
    Unavailable,
}

/// Internal result that keeps an implementation limit separate from a semantic rejection.
enum SemanticOutcome<T> {
    Available(T),
    Rejected,
    Unavailable,
}

/// Orchestrates one bounded trait-selection request over a shared semantic context.
///
/// It discovers native impl candidates, proves small concrete impl chains without constructing a
/// solver program, and falls back to Chalk for the remaining predicates and projections. Every
/// path works on trial inference state until the caller explicitly adopts a result.
pub struct TraitSelectionQuery<'query, D, I> {
    context: TyContext<'query, D, I>,
}

impl<'query, D, I> TraitSelectionQuery<'query, D, I>
where
    D: DefMapSource<Error = I::Error>,
    I: ItemStoreSource<'query>,
{
    pub fn new(context: TyContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Prove canonical clauses as one conjunction over the caller's inference variables.
    ///
    /// Related bounds cannot be submitted independently. In
    /// `I: Iterator<Item = T>, T: Copy`, both predicates must see the same existential `T`, and
    /// the returned table must preserve the equality learned from `Iterator::Item`.
    pub fn prove_clauses(
        &self,
        clauses: &[Clause],
        subst: &InferenceSubstitution,
        table: &InferenceTable,
    ) -> Result<TraitProof, I::Error> {
        self.prove_clauses_with_candidate_evidence(clauses, subst, table, CandidateEvidence::ROOT)
    }

    /// Prove predicates belonging to a native candidate without recursively selecting an impl
    /// already on the same proof path.
    ///
    /// A candidate's predicates may project through an unrelated predicate-free impl. That direct
    /// declaration is useful native evidence; candidates that need another proof remain together
    /// in the outer Chalk goal.
    fn prove_candidate_clauses(
        &self,
        clauses: &[Clause],
        subst: &InferenceSubstitution,
        table: &InferenceTable,
        active_impls: &[ImplRef],
    ) -> Result<TraitProof, I::Error> {
        self.prove_clauses_with_candidate_evidence(
            clauses,
            subst,
            table,
            CandidateEvidence::within(active_impls),
        )
    }

    /// Normalize declaration predicates, try the native proof forms, then fall back to Chalk.
    ///
    /// `candidate_evidence` controls only recursive associated-type normalization. Its active path
    /// enables cheap declaration projection without recursively building a second trait engine.
    fn prove_clauses_with_candidate_evidence(
        &self,
        clauses: &[Clause],
        subst: &InferenceSubstitution,
        table: &InferenceTable,
        candidate_evidence: CandidateEvidence<'_>,
    ) -> Result<TraitProof, I::Error> {
        if clauses.is_empty() {
            return Ok(TraitProof::Proven(table.clone()));
        }
        // Instantiate declaration parameters first, then normalize nested projections before a
        // later predicate consumes them. The entry point decides whether native candidate evidence
        // is available or whether this proof is already inside candidate selection.
        let mut table = table.clone();
        let mut normalized_clauses = Vec::with_capacity(clauses.len());
        for clause in clauses {
            let clause = subst.as_substitution().apply_clause(clause);
            let clause = match clause {
                Clause::Implemented(mut application) => {
                    let mut args = Vec::with_capacity(application.args.len());
                    for arg in application.args.iter() {
                        let arg = match arg {
                            GenericArg::Type(ty) => {
                                let (ty, next_table) = self.normalize_ty_with_candidate_evidence(
                                    ty,
                                    &table,
                                    candidate_evidence,
                                )?;
                                table = next_table;
                                GenericArg::Type(Box::new(ty))
                            }
                            GenericArg::Lifetime(_) | GenericArg::Const(_) => arg.clone(),
                        };
                        args.push(arg);
                    }
                    application.args = args.into();
                    Some(Clause::Implemented(application))
                }
                Clause::AliasEq { mut alias, ty } => {
                    let mut args = Vec::with_capacity(alias.args.len());
                    for arg in alias.args.iter() {
                        let arg = match arg {
                            GenericArg::Type(ty) => {
                                let (ty, next_table) = self.normalize_ty_with_candidate_evidence(
                                    ty,
                                    &table,
                                    candidate_evidence,
                                )?;
                                table = next_table;
                                GenericArg::Type(Box::new(ty))
                            }
                            GenericArg::Lifetime(_) | GenericArg::Const(_) => arg.clone(),
                        };
                        args.push(arg);
                    }
                    alias.args = args.into();
                    let (ty, next_table) =
                        self.normalize_ty_with_candidate_evidence(&ty, &table, candidate_evidence)?;
                    table = next_table;

                    // A declaration-owned associated equality is itself a projection proof, not
                    // merely a type containing projections. Resolve an exact selected value here
                    // so the native impl-chain prover can consume the remaining ordinary bounds.
                    // Ambiguous guidance may refine the table but keeps the equality for Chalk.
                    let projection = self.normalize_projection_once(
                        &alias,
                        &table,
                        candidate_evidence.native_only(),
                    )?;
                    if let Some(projection) = projection {
                        let (projected_ty, applicability, projected_table) =
                            projection.into_parts();
                        table = projected_table;
                        if table.try_unify(&projected_ty, &ty).is_err() {
                            return Ok(TraitProof::NoSolution);
                        }
                        if applicability == TraitApplicability::Yes && !ty.has_unknown() {
                            None
                        } else {
                            Some(Clause::AliasEq { alias, ty })
                        }
                    } else {
                        Some(Clause::AliasEq { alias, ty })
                    }
                }
            };
            if let Some(clause) = clause {
                normalized_clauses.push(table.canonicalize_clause(&clause));
            }
        }

        if normalized_clauses.is_empty() {
            return Ok(TraitProof::Proven(table));
        }

        if let Some(proof) = NativeProofQuery::new(self).prove(&normalized_clauses, &table)? {
            return Ok(proof);
        }

        let outcome = self.context.trait_selection().prove_clauses(
            self.context.item_paths(),
            self.context.crate_items(),
            self.context.lookup_index(),
            &normalized_clauses,
            &table,
        )?;
        Ok(match outcome {
            ChalkOutcome::Proven(table) => TraitProof::Proven(table),
            ChalkOutcome::Ambiguous(table) => TraitProof::Ambiguous(table),
            ChalkOutcome::NoSolution => TraitProof::NoSolution,
            ChalkOutcome::Unsupported | ChalkOutcome::Exhausted => TraitProof::Unavailable,
        })
    }

    /// Return the unique visible impl whose header fits and whose predicates can be proved.
    ///
    /// This is probe mode: every candidate gets a cloned inference table, and the caller's table
    /// remains unchanged even if a candidate would solve variables.
    ///
    /// Multiple distinct concrete selections become `ExpectedUnique::Ambiguous`. Speculative
    /// `Maybe` selections are used only when no concrete selection survives.
    pub fn probe(
        &self,
        goal: &TraitGoal,
        table: &InferenceTable,
    ) -> Result<ExpectedUnique<TraitSelection>, I::Error> {
        self.probe_with_completeness(goal, table)
            .map(|(selection, _)| selection)
    }

    /// Probe while preserving whether every matching native candidate reached a semantic answer.
    ///
    /// An empty complete result can terminate queries for a concrete type whose implementations
    /// are entirely represented by indexed impl headers. An incomplete result must remain
    /// distinguishable: bounded Chalk work may have declined a candidate without proving that the
    /// Rust goal has no solution.
    fn probe_with_completeness(
        &self,
        goal: &TraitGoal,
        table: &InferenceTable,
    ) -> Result<(ExpectedUnique<TraitSelection>, bool), I::Error> {
        self.probe_with_completeness_avoiding(goal, table, CandidateEvidence::ROOT)
    }

    /// Probe candidates while reserving active impls for the outer Chalk goal.
    fn probe_with_completeness_avoiding(
        &self,
        goal: &TraitGoal,
        table: &InferenceTable,
        candidate_evidence: CandidateEvidence<'_>,
    ) -> Result<(ExpectedUnique<TraitSelection>, bool), I::Error> {
        let active_impls = candidate_evidence.active_impls();
        // A goal that carries live inference or closure identity must be re-evaluated with its
        // owning table. Fully stable semantic goals cannot change the caller's table, so cache only
        // the selected impl/substitution and attach the caller's current table on a hit. Recursive
        // candidate proof cannot use this cache because a cached answer may itself be on the active
        // path.
        let cacheable = active_impls.is_empty() && goal.is_cache_stable();
        if cacheable
            && let Some(selection) = self.context.trait_selection().strict_selection(goal, table)
        {
            // Strict selections enter the cache only after every candidate was classified.
            return Ok((selection, true));
        }

        let plausible_impls = TraitCandidate::plausible_impls(
            self.context.item_paths(),
            self.context.lookup_index(),
            self.context.trait_selection(),
            goal,
            table,
        )?;

        let mut definite_selections = ExpectedUnique::new();
        let mut maybe_selections = ExpectedUnique::new();
        let mut fully_evaluated = true;
        for trait_impl in plausible_impls {
            let Some(candidate) = TraitCandidate::probe_impl(
                self.context.item_paths(),
                self.context.trait_selection(),
                goal,
                table,
                trait_impl,
            )?
            else {
                continue;
            };
            if active_impls.contains(&candidate.trait_impl.impl_ref) {
                // Returning an ordinary empty result here would incorrectly prove absence for a
                // nominal receiver. Mark the probe incomplete so projection normalization enters
                // Chalk, whose forest owns recursive semantic goals.
                crate::profile::metric::NATIVE_CANDIDATE_CYCLES.inc();
                return Ok((ExpectedUnique::new(), false));
            }
            let selection = match self.select_candidate(goal, candidate, candidate_evidence)? {
                SemanticOutcome::Available(selection) => selection,
                SemanticOutcome::Rejected => continue,
                SemanticOutcome::Unavailable => {
                    fully_evaluated = false;
                    continue;
                }
            };
            if selection.applicability == TraitApplicability::Yes {
                definite_selections.push(selection);
            } else {
                maybe_selections.push(selection);
            }
        }

        // A speculative header or ambiguous proof must not drown out a concrete result. This
        // ranking belongs to semantic selection; exploratory discovery exposes all candidates.
        let selection = if !definite_selections.is_empty() {
            definite_selections
        } else {
            maybe_selections
        };
        if cacheable && fully_evaluated {
            self.context
                .trait_selection()
                .remember_strict_selection(goal.clone(), &selection);
        }
        Ok((selection, fully_evaluated))
    }

    /// Prove an impl that receiver matching has already instantiated.
    ///
    /// Method lookup starts from one indexed impl and matches its `Self` header against the
    /// receiver. That match already supplies every substitution needed to instantiate the impl's
    /// own trait application, so rediscovering the same impl through native goal matching would be
    /// duplicate work. This entry point starts at the semantic boundary that remains: proving the
    /// instantiated predicates and associated-type constraints.
    ///
    /// Stable exact classifications are shared across fixed-point passes. Definite rejection and
    /// genuine proof ambiguity are cacheable; adapter limits remain an uncached `Maybe` so a later
    /// query can retry instead of treating bounded work exhaustion as a semantic fact.
    pub(crate) fn probe_instantiated_impl(
        &self,
        trait_impl: TraitImplRef,
        header: &crate::ImplHeader,
        subst: Substitution,
        table: &InferenceTable,
    ) -> Result<Option<TraitSelection>, I::Error> {
        let Some(mut trait_ref) = header.trait_ref.clone() else {
            return Ok(None);
        };
        trait_ref.application.args = trait_ref
            .application
            .args
            .iter()
            .map(|arg| subst.apply_arg(arg))
            .collect();
        trait_ref.associated_types = trait_ref
            .associated_types
            .into_iter()
            .map(|binding| AssocTypeBinding {
                associated_ty: binding.associated_ty,
                ty: subst.apply(&binding.ty),
            })
            .collect();
        let goal = TraitGoal::from_lowering(trait_ref);

        if let Some(applicability) = self
            .context
            .trait_selection()
            .exact_candidate_applicability(&goal, trait_impl)
        {
            return Ok(applicability.is_applicable().then(|| TraitSelection {
                trait_impl,
                subst: InferenceSubstitution::from_substitution(subst),
                applicability,
                table: table.clone(),
            }));
        }

        let candidate = TraitCandidate {
            trait_impl,
            subst: InferenceSubstitution::from_substitution(subst),
            applicability: TraitApplicability::Yes,
            table: table.clone(),
        };
        // Preserve the matched candidate for an editor-facing `Maybe` when the bounded adapter
        // cannot classify it. The trial starts from the caller's live inference state, so any
        // body-owned variables keep their original identities.
        let unavailable = candidate.clone();
        let outcome =
            self.select_candidate_with_header(&goal, candidate, header, CandidateEvidence::ROOT)?;
        let selection = match outcome {
            SemanticOutcome::Available(selection) => {
                self.context
                    .trait_selection()
                    .remember_exact_candidate_applicability(
                        &goal,
                        trait_impl,
                        selection.applicability,
                    );
                Some(selection)
            }
            SemanticOutcome::Rejected => {
                self.context
                    .trait_selection()
                    .remember_exact_candidate_applicability(
                        &goal,
                        trait_impl,
                        TraitApplicability::No,
                    );
                None
            }
            SemanticOutcome::Unavailable => Some(TraitSelection {
                trait_impl,
                subst: unavailable.subst,
                applicability: TraitApplicability::Maybe,
                table: unavailable.table,
            }),
        };
        Ok(selection)
    }

    /// Turn a native header match into semantic evidence by proving every remaining condition.
    fn select_candidate(
        &self,
        goal: &TraitGoal,
        candidate: TraitCandidate,
        candidate_evidence: CandidateEvidence<'_>,
    ) -> Result<SemanticOutcome<TraitSelection>, I::Error> {
        let Some(header) = self.context.trait_selection().impl_header_with(
            self.context.item_paths(),
            self.context.item_paths(),
            candidate.trait_impl.impl_ref,
        )?
        else {
            return Ok(SemanticOutcome::Unavailable);
        };
        self.select_candidate_with_header(goal, candidate, &header, candidate_evidence)
    }

    /// Prove a candidate whose canonical header is already available to the caller.
    fn select_candidate_with_header(
        &self,
        goal: &TraitGoal,
        candidate: TraitCandidate,
        header: &crate::ImplHeader,
        candidate_evidence: CandidateEvidence<'_>,
    ) -> Result<SemanticOutcome<TraitSelection>, I::Error> {
        let TraitCandidate {
            trait_impl,
            mut subst,
            mut applicability,
            mut table,
        } = candidate;

        // Candidate-aware projection is a declaration shortcut, not recursive trait solving.
        // A predicate-free impl can provide its associated value immediately. If this impl has
        // conditions of its own, keep the original projection equality in the caller's combined
        // Chalk goal; proving it here would branch candidate trees and materialize one program per
        // leaf.
        if !candidate_evidence.allows_solver_fallback() && !header.clauses.is_empty() {
            crate::profile::metric::NATIVE_CANDIDATE_PREDICATE_DECLINES.inc();
            return Ok(SemanticOutcome::Unavailable);
        }

        let generics = self
            .context
            .item_paths()
            .generics()
            .generics(GenericDefRef::Impl(trait_impl.impl_ref))?;
        subst.instantiate_missing_type_params(&mut table, &generics);

        if header.clauses.is_empty() {
            crate::profile::metric::PREDICATE_FREE_CANDIDATES.inc();
        } else {
            let active_impls = candidate_evidence.active_impls();
            let mut candidate_path = Vec::with_capacity(active_impls.len() + 1);
            candidate_path.extend_from_slice(active_impls);
            candidate_path.push(trait_impl.impl_ref);
            let predicate_applicability =
                self.prove_candidate_clauses(&header.clauses, &subst, &table, &candidate_path)?;
            match predicate_applicability {
                TraitProof::Proven(proven_table) => table = proven_table,
                TraitProof::Ambiguous(guided_table) => {
                    if let Some(guided_table) = guided_table {
                        table = guided_table;
                    }
                    applicability = applicability.and(TraitApplicability::Maybe);
                }
                TraitProof::NoSolution => return Ok(SemanticOutcome::Rejected),
                TraitProof::Unavailable => {
                    return Ok(SemanticOutcome::Unavailable);
                }
            };
        }

        match self.apply_assoc_type_constraints(
            goal,
            trait_impl.impl_ref,
            &subst,
            &mut table,
            &mut applicability,
        )? {
            SemanticOutcome::Available(()) => {}
            SemanticOutcome::Rejected => return Ok(SemanticOutcome::Rejected),
            SemanticOutcome::Unavailable => return Ok(SemanticOutcome::Unavailable),
        }

        Ok(SemanticOutcome::Available(TraitSelection {
            trait_impl,
            subst,
            applicability,
            table,
        }))
    }

    fn apply_assoc_type_constraints(
        &self,
        goal: &TraitGoal,
        impl_ref: ImplRef,
        subst: &InferenceSubstitution,
        table: &mut InferenceTable,
        applicability: &mut TraitApplicability,
    ) -> Result<SemanticOutcome<()>, I::Error> {
        if !goal.has_assoc_type_constraints() {
            return Ok(SemanticOutcome::Available(()));
        }

        let projection_goal = goal.without_assoc_type_constraints();
        for constraint in goal.assoc_type_constraints() {
            let projection = self.context.trait_selection().normalize_assoc_type(
                self.context.item_paths(),
                self.context.crate_items(),
                self.context.lookup_index(),
                &projection_goal,
                constraint.associated_ty,
                Some((impl_ref, subst)),
                table,
            )?;
            let projection = match projection {
                ChalkOutcome::Proven(projection) => projection,
                ChalkOutcome::Ambiguous(Some(projection)) => projection,
                ChalkOutcome::NoSolution => return Ok(SemanticOutcome::Rejected),
                ChalkOutcome::Ambiguous(None)
                | ChalkOutcome::Unsupported
                | ChalkOutcome::Exhausted => return Ok(SemanticOutcome::Unavailable),
            };

            let (projection_ty, mut projection_table) =
                self.normalize_ty(&projection.ty, &projection.table)?;
            if projection_table
                .try_unify(&projection_ty, constraint.ty)
                .is_err()
            {
                return Ok(SemanticOutcome::Rejected);
            }
            *table = projection_table;
            *applicability = applicability.and(projection.applicability);
        }

        Ok(SemanticOutcome::Available(()))
    }
}

#[cfg(test)]
mod tests;
