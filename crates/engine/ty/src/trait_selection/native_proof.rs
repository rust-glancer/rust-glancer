//! Small native proofs that avoid materializing a complete Chalk program.
//!
//! This is deliberately not a second general-purpose trait solver. It handles compiler-known
//! closure facts and follows concrete indexed impl chains while they remain unambiguous and fully
//! representable. Associated equalities, open inference, unsupported headers, and recursive cycles
//! fall back to the bounded Chalk adapter owned by [`TraitSelectionQuery`].

use rg_def_map::DefMapSource;
use rg_ir_model::{GenericDefRef, TraitApplicability};
use rg_item_tree::LangItem;
use rg_semantic_ir::ItemStoreSource;
use rg_std::ExpectedUnique;

use super::{
    TraitGoal, TraitProof, TraitSelectionQuery, candidate::TraitCandidate, matcher::TraitSelfHead,
};
use crate::inference::InferenceTable;
use crate::{Clause, GenericArg, TraitApplication, Ty};

// Native proof is only a shortcut around the bounded solver. Keep recursive impl chains bounded so
// a long or cyclic declaration graph can decline the shortcut instead of monopolizing the query.
const MAX_PROOF_DEPTH: usize = 32;

/// Owns the recursion state for one bounded native proof attempt.
///
/// The active-goal stack and depth limit stay here instead of leaking into the trait-selection
/// orchestrator. Crossing an unsupported shape or recursion boundary returns no native answer, so
/// the caller can submit the same semantic obligation to Chalk.
pub(super) struct NativeProofQuery<'owner, 'query, D, I> {
    selection: &'owner TraitSelectionQuery<'query, D, I>,
    active: Vec<TraitApplication>,
}

impl<'owner, 'query, D, I> NativeProofQuery<'owner, 'query, D, I>
where
    D: DefMapSource<Error = I::Error>,
    I: ItemStoreSource<'query>,
{
    pub(super) fn new(selection: &'owner TraitSelectionQuery<'query, D, I>) -> Self {
        Self {
            selection,
            active: Vec::new(),
        }
    }

    /// Try every native proof form before the orchestrator falls back to Chalk.
    pub(super) fn prove(
        mut self,
        clauses: &[Clause],
        table: &InferenceTable,
    ) -> Result<Option<TraitProof>, I::Error> {
        if let Some(proof) = self.prove_closure_callable_clauses(clauses, table) {
            return Ok(Some(proof));
        }
        self.prove_from_impls(clauses, table)
    }

    /// Prove `Fn*` obligations directly from one closure's inference-aware signature.
    ///
    /// Capture analysis is not modeled yet, so the Chalk adapter classifies every closure as
    /// `Fn`, which also satisfies `FnMut` and `FnOnce`. Keep that same boundary here and decline
    /// the whole shortcut if any clause is not a compiler-known callable fact about a closure.
    fn prove_closure_callable_clauses(
        &self,
        clauses: &[Clause],
        table: &InferenceTable,
    ) -> Option<TraitProof> {
        let lookup_index = self.selection.context.lookup_index();
        let callable_traits = [
            lookup_index.lang_trait(LangItem::Fn),
            lookup_index.lang_trait(LangItem::FnMut),
            lookup_index.lang_trait(LangItem::FnOnce),
        ];
        let output_alias = lookup_index.lang_type_alias(LangItem::FnOnceOutput)?;
        let mut table = table.clone();

        for clause in clauses {
            let (args, output) = match clause {
                Clause::Implemented(application)
                    if callable_traits.contains(&Some(application.def)) =>
                {
                    (&application.args, None)
                }
                Clause::AliasEq { alias, ty } if alias.associated_ty == output_alias => {
                    (&alias.args, Some(ty))
                }
                Clause::Implemented(_) | Clause::AliasEq { .. } => return None,
            };
            let [GenericArg::Type(self_ty), GenericArg::Type(argument_tuple)] = args.as_slice()
            else {
                return None;
            };
            let Ty::Closure(closure) = table.canonicalize(self_ty) else {
                return None;
            };

            let signature_args = Ty::tuple(closure.params.clone());
            if table.try_unify(&signature_args, argument_tuple).is_err()
                || output.is_some_and(|output| table.try_unify(&closure.ret, output).is_err())
            {
                return Some(TraitProof::NoSolution);
            }
        }

        Some(TraitProof::Proven(table))
    }

    /// Prove a concrete conjunction through indexed impl headers and their concrete predicates.
    ///
    /// Associated equalities, inference, callables, ambiguity, and recursive cycles decline the
    /// entire native proof and leave it to Chalk.
    fn prove_from_impls(
        &mut self,
        clauses: &[Clause],
        table: &InferenceTable,
    ) -> Result<Option<TraitProof>, I::Error> {
        let mut table = table.clone();
        for clause in clauses {
            let Clause::Implemented(application) = clause else {
                return Ok(None);
            };
            let has_live_vars = application.args.iter().any(GenericArg::has_var);
            if has_live_vars {
                let Some(self_ty) = application.self_ty() else {
                    return Ok(None);
                };
                if self_ty.has_var()
                    || application
                        .args
                        .iter()
                        .any(|arg| arg.has_unknown() || arg.has_closure() || arg.has_projection())
                {
                    return Ok(None);
                }
                let Some(proof) = self.prove_definitive_absence(application, &table)? else {
                    return Ok(None);
                };
                return Ok(Some(proof));
            }
            if application
                .args
                .iter()
                .any(|arg| arg.has_unknown() || arg.has_closure() || arg.has_projection())
            {
                return Ok(None);
            }
            let Some(proof) = self.prove_application(application, &table)? else {
                return Ok(None);
            };
            match proof {
                TraitProof::Proven(proven_table) => table = proven_table,
                TraitProof::NoSolution => return Ok(Some(TraitProof::NoSolution)),
                TraitProof::Ambiguous(_) | TraitProof::Unavailable => return Ok(None),
            }
        }
        Ok(Some(TraitProof::Proven(table)))
    }

    /// Reject a concrete-self goal only when Chalk would omit every plausible native impl.
    ///
    /// This is useful for goals such as `Tuple: PartialEq<?Rhs>`: the open input prevents a native
    /// proof, but building the solver program cannot help when all matching tuple declarations have
    /// unknown self types and therefore cannot become Chalk impl datums.
    fn prove_definitive_absence(
        &self,
        application: &TraitApplication,
        table: &InferenceTable,
    ) -> Result<Option<TraitProof>, I::Error> {
        let Some(self_ty) = application.self_ty() else {
            return Ok(None);
        };
        let Some(goal_head) = TraitSelfHead::from_ty(&table.resolve_root_var(self_ty)) else {
            return Ok(None);
        };
        if matches!(
            goal_head,
            TraitSelfHead::Closure(_) | TraitSelfHead::FnDef(_) | TraitSelfHead::FnPointer(_)
        ) {
            return Ok(None);
        }
        let goal = TraitGoal {
            application: application.clone(),
            associated_types: Vec::new(),
        };
        for candidate in TraitCandidate::probe_all(
            self.selection.context.item_paths(),
            self.selection.context.lookup_index(),
            self.selection.context.trait_selection(),
            &goal,
            table,
        )? {
            let Some(header) = self.selection.context.trait_selection().impl_header_with(
                self.selection.context.item_paths(),
                self.selection.context.item_paths(),
                candidate.trait_impl.impl_ref,
            )?
            else {
                return Ok(None);
            };
            if !header.self_ty.has_unknown() {
                return Ok(None);
            }
        }
        Ok(Some(TraitProof::NoSolution))
    }

    /// Guard recursion for one concrete application, then classify its matching impls.
    ///
    /// A cycle or depth limit is an adapter boundary, not a Rust-level rejection, so it declines
    /// native proof and lets Chalk classify the original goal.
    fn prove_application(
        &mut self,
        application: &TraitApplication,
        table: &InferenceTable,
    ) -> Result<Option<TraitProof>, I::Error> {
        if self.active.len() >= MAX_PROOF_DEPTH || self.active.contains(application) {
            return Ok(None);
        }
        let Some(self_ty) = application.self_ty() else {
            return Ok(None);
        };
        let Some(goal_head) = TraitSelfHead::from_ty(&table.resolve_root_var(self_ty)) else {
            return Ok(None);
        };
        // These shapes have compiler-built callable clauses outside the ordinary impl index.
        if matches!(
            goal_head,
            TraitSelfHead::Closure(_) | TraitSelfHead::FnDef(_) | TraitSelfHead::FnPointer(_)
        ) {
            return Ok(None);
        }

        self.active.push(application.clone());
        let result = self.prove_application_candidates(application, goal_head, table);
        self.active.pop();
        result
    }

    /// Prove one application through its matching structural and blanket impls.
    ///
    /// A unique exact self-head impl wins over blanket fallbacks. If any plausible candidate cannot
    /// be represented or the surviving proofs are ambiguous, return no native answer rather than
    /// turning an implementation limit into `NoSolution`.
    fn prove_application_candidates(
        &mut self,
        application: &TraitApplication,
        goal_head: TraitSelfHead,
        table: &InferenceTable,
    ) -> Result<Option<TraitProof>, I::Error> {
        let goal = TraitGoal {
            application: application.clone(),
            associated_types: Vec::new(),
        };
        let candidates = TraitCandidate::probe_all(
            self.selection.context.item_paths(),
            self.selection.context.lookup_index(),
            self.selection.context.trait_selection(),
            &goal,
            table,
        )?;
        let mut exact = ExpectedUnique::new();
        let mut all = ExpectedUnique::new();
        let mut has_unavailable = false;

        for candidate in candidates {
            let Some(header) = self.selection.context.trait_selection().impl_header_with(
                self.selection.context.item_paths(),
                self.selection.context.item_paths(),
                candidate.trait_impl.impl_ref,
            )?
            else {
                has_unavailable = true;
                continue;
            };
            if candidate.applicability != TraitApplicability::Yes {
                // This is the same unsupported boundary as Chalk program materialization.
                if header.self_ty.has_unknown() {
                    continue;
                }
                has_unavailable = true;
                continue;
            }

            let mut candidate_table = candidate.table;
            let mut subst = candidate.subst;
            let generics = self
                .selection
                .context
                .item_paths()
                .generics()
                .generics(GenericDefRef::Impl(candidate.trait_impl.impl_ref))?;
            subst.instantiate_missing_type_params(&mut candidate_table, &generics);
            let instantiated_clauses = header
                .clauses
                .iter()
                .map(|clause| subst.as_substitution().apply_clause(clause))
                .map(|clause| candidate_table.canonicalize_clause(&clause))
                .collect::<Vec<_>>();
            let proven_table = if instantiated_clauses.is_empty() {
                Some(candidate_table)
            } else {
                match self.prove_from_impls(&instantiated_clauses, &candidate_table)? {
                    Some(TraitProof::Proven(table)) => Some(table),
                    Some(TraitProof::NoSolution) => None,
                    Some(TraitProof::Ambiguous(_) | TraitProof::Unavailable) | None => {
                        has_unavailable = true;
                        continue;
                    }
                }
            };
            let Some(proven_table) = proven_table else {
                continue;
            };
            let proof = (candidate.trait_impl.impl_ref, proven_table);
            all.push(proof.clone());
            if TraitSelfHead::from_ty(&header.self_ty) == Some(goal_head) {
                exact.push(proof);
            }
        }

        // Prefer a proven structural impl over blanket fallbacks. Coherence prevents another
        // overlapping impl from becoming applicable, while an unsupported blanket must not force
        // the full Chalk universe after the exact declaration has already supplied the proof.
        let table = match exact {
            ExpectedUnique::One((_, table)) => table,
            ExpectedUnique::Ambiguous => return Ok(None),
            ExpectedUnique::Empty if has_unavailable => return Ok(None),
            ExpectedUnique::Empty => match all {
                ExpectedUnique::One((_, table)) => table,
                ExpectedUnique::Ambiguous => return Ok(None),
                ExpectedUnique::Empty => return Ok(Some(TraitProof::NoSolution)),
            },
        };
        Ok(Some(TraitProof::Proven(table)))
    }
}
