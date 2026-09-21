//! Call-signature inference over canonical semantic signatures.
//!
//! Call lookup chooses a function and supplies receiver/impl substitutions. This layer gives the
//! selected function's own parameters live inference variables, binds arguments and return
//! evidence, and submits its already-lowered clauses. Declaration syntax is not projected again.

use anyhow::Context as _;
use rg_def_map::DefMapSource;
use rg_ir_model::{ExprId, GenericDefRef, GenericParamRef};
use rg_item_tree::FunctionQualifiers;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{GenericParamSource, ItemStoreSource};
use rg_ty::{
    Clause, GenericArg, Substitution, Ty,
    inference::{InferenceSubstitution, InferenceTable},
    lowering::CallableSignature,
    trait_selection::TraitProof,
};

use crate::{CallFacts, resolution::query::CallSelfSource};

use super::{InferenceContext, fulfill::DeferredKind};
use crate::body::ExprKind;

/// Call-owned signature projection and live generic slots.
///
/// Inferable type arguments, including written `_`, get stable variables. Pending obligations
/// and return projections keep those variables even when evidence arrives after the call's syntax.
pub(super) struct CallInferenceState {
    function: rg_ir_model::FunctionRef,
    generic_params: Vec<GenericParamRef>,
    signature: CallableSignature,
    subst: InferenceSubstitution,
    first_written_param_idx: usize,
    receiver_ty: Option<Ty>,
    return_projection_complete: bool,
    return_projections: Vec<(rg_ty::ProjectionTy, Ty)>,
    /// Canonical return type used by the last associated-type normalization attempt.
    ///
    /// `Iterator::Item` may remain unresolved at several checkpoints. The call retries only when
    /// inference changes this input key; `None` means normalization has not been attempted yet.
    last_return_projection_input: Option<Ty>,
    generic_obligations_complete: bool,
    last_obligation_input: Option<Vec<Clause>>,
}

impl CallInferenceState {
    /// Return the function whose signature owns this call's live inference state.
    pub(super) fn function(&self) -> rg_ir_model::FunctionRef {
        self.function
    }

    /// Completion means the call needs no further proof or return normalization. Its types can
    /// still contain variables shared with arguments or expectations; ordinary unification will
    /// carry any later evidence through those links.
    pub(super) fn is_complete(&self) -> bool {
        self.return_projection_complete && self.generic_obligations_complete
    }

    /// An associated return value can still normalize to `!`. Keep its destination separate from
    /// a coercion target until normalization determines the type it actually produces.
    pub(super) fn result_is_pending(&self, table: &InferenceTable, ty: &Ty) -> bool {
        !self.return_projection_complete
            && self
                .return_projections
                .iter()
                .any(|(_, destination)| table.resolve_root_var(destination) == *ty)
    }

    /// Definite guidance can change a call's own inputs while its proof remains ambiguous.
    /// Schedule another fulfillment checkpoint only when that committed guidance changed the
    /// actual question, rather than waiting for an unrelated expression to make progress.
    pub(super) fn needs_fulfillment(&self, table: &InferenceTable) -> bool {
        if !self.generic_obligations_complete {
            let input = self.obligation_input(table);
            if self.last_obligation_input.as_ref() != Some(&input) {
                return true;
            }
        }
        if !self.return_projection_complete {
            let input = self.return_projection_input(table);
            if self.last_return_projection_input.as_ref() != Some(&input) {
                return true;
            }
        }
        false
    }

    /// Canonical clauses let scheduling and proof attempts agree on what counts as new evidence.
    fn obligation_input(&self, table: &InferenceTable) -> Vec<Clause> {
        self.signature
            .clauses
            .iter()
            .map(|clause| {
                table.canonicalize_clause(&self.subst.as_substitution().apply_clause(clause))
            })
            .collect()
    }

    /// Canonical return shape used to decide whether associated projections can make progress.
    fn return_projection_input(&self, table: &InferenceTable) -> Ty {
        table.canonicalize(&self.subst.as_substitution().apply(&self.signature.ret))
    }

    /// Include generic bindings in the call's retry input. A bound can mention a parameter that
    /// does not appear in any written argument or in the result type.
    pub(super) fn input(&self) -> Ty {
        Ty::tuple(
            self.generic_params
                .iter()
                .filter_map(|param| {
                    self.subst
                        .as_substitution()
                        .get(*param)
                        .and_then(GenericArg::as_ty)
                        .cloned()
                })
                .collect(),
        )
    }

    /// Preserve the semantic spelling of an associated value that remained ambiguous. This is
    /// finalization only: known expected types and successfully normalized destinations win.
    pub(super) fn finish_projections(&self, table: &mut InferenceTable) {
        for (projection, destination) in &self.return_projections {
            if matches!(table.resolve_root_var(destination), Ty::InferVar { .. }) {
                table.unify(
                    destination,
                    &Ty::Alias(rg_ty::AliasTy::Projection(projection.clone())),
                );
            }
        }
    }

    /// Collapse the call-owned inference substitution into its persistent semantic form.
    pub(super) fn finalize(&self, table: &InferenceTable) -> CallFacts {
        let generic_args = self
            .subst
            .finalize_args(table, self.generic_params.iter().copied());
        CallFacts::new(self.function, generic_args)
    }
}

/// A selected signature whose arguments can now be inferred against its parameter types.
/// Completion binds their evidence, fulfills clauses, and retains the state for later projections.
pub(super) struct PreparedCall {
    call: ExprId,
    state: CallInferenceState,
}

impl PreparedCall {
    /// Return the selected signature's expected types for the written arguments.
    ///
    /// A malformed or incomplete call has no positional correspondence, so an arity mismatch
    /// leaves expectations unknown rather than applying them to the wrong expressions.
    fn argument_expected_ty(&self, index: usize, arg_count: usize) -> Ty {
        let written_params = self
            .state
            .signature
            .params
            .get(self.state.first_written_param_idx..)
            .unwrap_or_default();
        if written_params.len() != arg_count {
            return Ty::Unknown;
        }
        self.state
            .subst
            .as_substitution()
            .apply(&written_params[index])
    }
}

impl<'query, D, I> InferenceContext<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    /// Calls establish closure signatures early, then process ordinary arguments and fulfill
    /// their bounds before consuming closure bodies. Those bodies are still visited exactly once.
    /// For example, in `apply(|user| user.name, user)`, the second argument and a callable bound
    /// can tell us the closure parameter's type before we attempt the field lookup in its body.
    pub(super) fn infer_call(
        &mut self,
        call: ExprId,
        args: &[ExprId],
        receiver: Option<ExprId>,
        expected: &Ty,
    ) -> anyhow::Result<()> {
        for arg in args {
            self.prepare_closure(*arg);
        }
        let lookup_input = receiver.map(|expr| {
            self.inference
                .table()
                .canonicalize(&self.inference.expr_ty(expr))
        });
        let transfer = self
            .prepare_call(call, receiver)
            .context("select call signature")?;
        self.inference.expr_slot(call);
        // A selected generic signature can use the expectation to infer its type arguments.
        // An unresolved call or root projection may instead turn out to return `!`; its expected
        // type is applied by the expression's coercion after the pending work is registered.
        if transfer.as_ref().is_some_and(|transfer| {
            !transfer.state.result_is_pending(
                self.inference.table(),
                &self.inference.root_resolved_expr_ty(call),
            )
        }) {
            self.coerce_expr_ty(call, expected);
        }
        for (index, arg) in args.iter().enumerate() {
            if matches!(
                self.body.expr_unchecked(*arg).kind,
                ExprKind::Closure { .. }
            ) {
                continue;
            }
            let expected = transfer
                .as_ref()
                .map(|transfer| transfer.argument_expected_ty(index, args.len()))
                .unwrap_or(Ty::Unknown);
            self.infer_expr(*arg, &expected)
                .context("infer call argument")?;
        }
        let selected = transfer.is_some();
        if let Some(transfer) = transfer {
            self.finish_call(transfer, args)
                .context("apply call signature")?;
        }
        let pending = DeferredKind::Call { call };
        let receiver_changed = lookup_input
            != receiver.map(|expr| {
                self.inference
                    .table()
                    .canonicalize(&self.inference.expr_ty(expr))
            });
        if !selected && receiver_changed {
            // Ordinary argument inference can refine the same local used as this receiver.
            self.run_or_defer(pending)
                .context("retry call after argument inference")?;
        } else {
            self.defer(pending, None);
        }
        // Callable bounds can now constrain the prepared closure signatures. Make that evidence
        // available before the closures introduce their parameter bindings and walk their bodies.
        self.fulfill_pending()
            .context("fulfill argument obligations")?;
        for arg in args {
            if matches!(
                self.body.expr_unchecked(*arg).kind,
                ExprKind::Closure { .. }
            ) {
                self.infer_expr(*arg, &Ty::Unknown)
                    .context("infer closure argument")?;
            }
        }
        // Closure return types can unlock more obligations, such as the item type of a mapped
        // iterator. Give pending operations a chance to use that evidence before leaving the call.
        self.fulfill_pending()
            .context("fulfill closure obligations")?;
        Ok(())
    }

    /// Select a target and instantiate its signature once, before checking the arguments.
    pub(super) fn prepare_call(
        &mut self,
        call: ExprId,
        receiver: Option<ExprId>,
    ) -> Result<Option<PreparedCall>, PackageStoreError> {
        crate::profile::metric::CALL_ATTEMPTS.inc();
        let mut state = if let Some(state) = self.inference.take_call_inference(call) {
            // A retry resumes the selected signature. Allocating its generic slots again would
            // disconnect evidence already shared with argument expressions and the result.
            state
        } else {
            let calls = self.context.calls();
            // Only ordinary calls need the callee's name resolution. Semantic lookup receives
            // that live fact directly, alongside the receiver type evidence below.
            let callee_resolution = match self.body.expr_unchecked(call).kind {
                ExprKind::Call {
                    callee: Some(callee),
                    ..
                } => Some(self.inference.expr_resolution(callee)),
                _ => None,
            };
            // Keep nested slots in the selected receiver substitution so later arguments can
            // still constrain them. Predicate proof receives the owning table separately and can
            // canonicalize those slots without severing their connection to body inference.
            let receiver_ty =
                receiver.map(|receiver| self.inference.root_resolved_expr_ty(receiver));
            let target = calls.target(
                call,
                callee_resolution,
                receiver_ty.as_ref(),
                self.inference.table(),
            )?;
            // Structural lookup keeps live variables in successful substitutions. If a repeated
            // impl parameter only matches after its slots were solved, retry this one lookup with
            // the resolved shape, as for `Pair<?T, u8>` against `impl<T> Pair<T, T>`.
            let target = match target {
                Some(target) => Some(target),
                None => match &receiver_ty {
                    Some(ty) => {
                        let resolved = self.inference.table.canonicalize(ty);
                        if resolved != *ty {
                            calls.target(
                                call,
                                callee_resolution,
                                Some(&resolved),
                                self.inference.table(),
                            )?
                        } else {
                            None
                        }
                    }
                    None => None,
                },
            };
            let Some(mut target) = target else {
                return Ok(None);
            };
            if let Some(selection) = target.trait_selection.take() {
                // Candidate proof is transactional until lookup finds one definite target. Commit
                // its table now so equality evidence used to prove the impl remains true while
                // subsequent inference uses the selected signature.
                self.inference.table = selection.table;
            }
            let function = target.function();
            let first_written_param_idx = target.first_written_param_idx();
            let signature =
                self.context
                    .signatures()
                    .function(function)?
                    .unwrap_or(CallableSignature {
                        params: Vec::new(),
                        ret: Ty::Unknown,
                        clauses: Vec::new(),
                        qualifiers: FunctionQualifiers::default(),
                    });
            let generics = self
                .context
                .item_paths()
                .generics()
                .generics(GenericDefRef::Function(function))?;

            // Written arguments are lowered once, in call-site scope. The shared lowerer gives
            // omitted function types and explicit `_` their live variables; parent placeholders
            // must not replace receiver evidence selected by lookup.
            let explicit = if target.explicit_args().is_empty() {
                None
            } else {
                Some(
                    self.context
                        .type_refs(target.site_scope())
                        .resolve_generic_args_for(
                            &generics,
                            target.explicit_args(),
                            Some(&mut self.inference.table),
                        )?,
                )
            };
            let (mut base, receiver_ty) = match target.self_source {
                CallSelfSource::None => (Substitution::new(), None),
                CallSelfSource::TypePrefix(context) | CallSelfSource::Receiver(context) => {
                    (context.subst, Some(context.self_ty))
                }
            };
            if let Some(self_ty) = &receiver_ty
                && let Some(self_param) = generics.iter().find_map(|param| {
                    matches!(param.source(), GenericParamSource::TraitSelf).then_some(param.param())
                })
            {
                base.push(self_param, GenericArg::Type(Box::new(self_ty.clone())));
            }
            if let Some(args) = explicit {
                for (param, arg) in generics
                    .iter_self()
                    .zip(args.iter().skip(generics.parent_len()))
                {
                    base.push(param.param(), arg.clone());
                }
            }
            for param in generics.iter() {
                if let GenericParamRef::Type(param) = param.param()
                    && base
                        .get(GenericParamRef::Type(param))
                        .and_then(GenericArg::as_ty)
                        .is_none_or(|ty| matches!(ty, Ty::Unknown))
                {
                    base.push(
                        GenericParamRef::Type(param),
                        GenericArg::Type(Box::new(self.inference.table.new_type_var())),
                    );
                }
            }
            CallInferenceState {
                function,
                generic_params: generics.iter().map(|param| param.param()).collect(),
                signature,
                subst: InferenceSubstitution::from_substitution(base),
                first_written_param_idx,
                receiver_ty,
                return_projection_complete: false,
                return_projections: Vec::new(),
                last_return_projection_input: None,
                generic_obligations_complete: false,
                last_obligation_input: None,
            }
        };

        // Install the return shape before argument expectations. Chained calls later in the same
        // expression walk can then use it as receiver evidence.
        if !state.return_projection_complete {
            let return_ty = state.subst.as_substitution().apply(&state.signature.ret);
            if return_ty.has_projection() {
                // Keep the surrounding shape usable while each associated value has its own
                // live destination. Consumers can share those destinations before normalization.
                if state.return_projections.is_empty() {
                    let (ty, projections) =
                        self.inference.table.instantiate_projections(&return_ty);
                    state.return_projections = projections;
                    self.inference.set_expr_infer_ty(call, ty);
                }
            } else if return_ty.has_var() {
                self.inference.set_expr_infer_ty(call, return_ty);
            } else if return_ty.has_unknown() {
                self.inference
                    .instantiate_expr_nested_unknown_ty(call, &return_ty);
            } else {
                self.inference.set_expr_infer_ty(call, return_ty);
            }
        }

        Ok(Some(PreparedCall { call, state }))
    }

    /// Connect the inferred arguments, receiver, and result with the selected signature.
    pub(super) fn finish_call(
        &mut self,
        mut transfer: PreparedCall,
        args: &[ExprId],
    ) -> Result<(), PackageStoreError> {
        // Bind argument evidence using the selected substitution. Use the adjusted receiver
        // from lookup: an inserted autoref is not equality with the written receiver expression.
        if transfer.state.first_written_param_idx == 1
            && let Some(receiver_ty) = &transfer.state.receiver_ty
            && let Some(receiver_param) = transfer.state.signature.params.first()
        {
            let receiver_pattern = match receiver_param {
                Ty::Reference { inner, .. } => inner.as_ref(),
                receiver_param => receiver_param,
            };
            transfer
                .state
                .subst
                .bind_ty(&mut self.inference.table, receiver_pattern, receiver_ty);
        }

        for (param, arg) in transfer
            .state
            .signature
            .params
            .iter()
            .skip(transfer.state.first_written_param_idx)
            .zip(args)
        {
            let evidence = self.inference.root_resolved_expr_ty(*arg);
            transfer
                .state
                .subst
                .bind_ty(&mut self.inference.table, param, &evidence);
        }
        let return_evidence = self.inference.expr_ty(transfer.call);
        if !matches!(return_evidence, Ty::Unknown) {
            transfer.state.subst.bind_ty(
                &mut self.inference.table,
                &transfer.state.signature.ret,
                &return_evidence,
            );
        }

        // Apply argument expectations even when the target only became selectable later. Their
        // types already retain structural relationships, so this needs no syntax traversal.
        for (arg, param) in args.iter().zip(
            transfer
                .state
                .signature
                .params
                .iter()
                .skip(transfer.state.first_written_param_idx),
        ) {
            let expected = transfer.state.subst.as_substitution().apply(param);
            self.coerce_expr_ty(*arg, &expected);
        }
        if transfer.state.first_written_param_idx == 1
            && let Some(receiver_ty) = &transfer.state.receiver_ty
            && let Some(receiver_param) = transfer.state.signature.params.first()
        {
            // Method-call syntax supplies the receiver before Rust inserts `&self`/`&mut self`.
            let receiver_pattern = match receiver_param {
                Ty::Reference { inner, .. } => inner.as_ref(),
                receiver_param => receiver_param,
            };
            let expected_receiver_ty = transfer
                .state
                .subst
                .as_substitution()
                .apply(receiver_pattern);
            self.inference
                .constrain_infer_tys(receiver_ty, &expected_receiver_ty);
        }

        self.solve_generic_trait_obligations(&mut transfer.state)?;
        self.finish_return_ty(transfer.call, &mut transfer.state)?;
        self.inference
            .set_call_inference(transfer.call, transfer.state);
        Ok(())
    }

    /// Submit the selected signature's canonical clauses with live call substitutions applied.
    fn solve_generic_trait_obligations(
        &mut self,
        state: &mut CallInferenceState,
    ) -> Result<(), PackageStoreError> {
        if state.generic_obligations_complete {
            return Ok(());
        }

        // A selected call obligation matters to this layer while it can still constrain a
        // body-owned inference slot, closure identity, or unresolved semantic shape. Fully settled
        // predicates are type-checking facts; proving them cannot change any Body IR type, so
        // eager indexing should not submit them to Chalk merely to rediscover that the
        // already-selected call is valid.
        //
        // `Unknown` and projections deliberately remain pending. Their producer may become known
        // at a later checkpoint, at which point the same obligation can carry useful evidence
        // into the call-owned substitution. Marking either shape complete would freeze calls such
        // as `map(...).collect::<Vec<_>>()` before `Map::Item` has been projected.
        let is_unsettled =
            |ty: &Ty| ty.has_var() || ty.has_closure() || ty.has_unknown() || ty.has_projection();
        let input = state.obligation_input(&self.inference.table);
        if state.last_obligation_input.as_ref() == Some(&input) {
            return Ok(());
        }
        state.last_obligation_input = Some(input.clone());
        let needs_body_inference = input.into_iter().any(|clause| match clause {
            Clause::Implemented(application) => application
                .args
                .iter()
                .any(|arg| arg.as_ty().is_some_and(is_unsettled)),
            Clause::AliasEq { alias, ty } => {
                is_unsettled(&ty)
                    || alias
                        .args
                        .iter()
                        .any(|arg| arg.as_ty().is_some_and(is_unsettled))
            }
        });
        if !needs_body_inference {
            state.generic_obligations_complete = true;
            return Ok(());
        }

        crate::profile::metric::OBLIGATION_ATTEMPTS.inc();
        let proof = self.context.trait_selection().prove_clauses(
            &state.signature.clauses,
            &state.subst,
            &self.inference.table,
        )?;
        state.generic_obligations_complete = match proof {
            TraitProof::Proven(table) => {
                self.inference.table = table;
                true
            }
            TraitProof::Ambiguous(Some(table)) => {
                // Chalk's definite guidance is an equality every possible solution shares. It is
                // safe inference evidence even though it does not prove that the obligation will
                // ultimately hold. Keep the obligation pending and retry after that evidence has
                // propagated through the body.
                self.inference.table = table;
                false
            }
            TraitProof::Ambiguous(None) | TraitProof::NoSolution | TraitProof::Unavailable => false,
        };
        Ok(())
    }

    /// Refine the return after argument binding, or normalize its registered associated values.
    fn finish_return_ty(
        &mut self,
        call: ExprId,
        state: &mut CallInferenceState,
    ) -> Result<(), PackageStoreError> {
        if state.return_projection_complete {
            return Ok(());
        }
        // Preparation gives every associated projection a stable destination. Without any,
        // retain the substituted result directly; no normalization or trial table is needed.
        if state.return_projections.is_empty() {
            let return_ty = state.subst.as_substitution().apply(&state.signature.ret);
            debug_assert!(!return_ty.has_projection());
            state.return_projection_complete = true;
            self.inference.set_call_return_ty(call, return_ty);
            return Ok(());
        }

        // A projection can remain ambiguous at several checkpoints. Retry only after the
        // variables inside its input have gained evidence; unrelated body progress cannot change
        // this projection's candidate set. Canonicalization is deliberately only the comparison
        // key—the registered projections and table below keep their live variable connections.
        let projection_input = state.return_projection_input(&self.inference.table);
        if state.last_return_projection_input.as_ref() == Some(&projection_input) {
            return Ok(());
        }
        state.last_return_projection_input = Some(projection_input);

        let mut complete = true;
        for (projection, destination) in &state.return_projections {
            let source = Ty::Alias(rg_ty::AliasTy::Projection(projection.clone()));
            let (ty, table) = self
                .context
                .trait_selection()
                .normalize_ty(&source, &self.inference.table)?;
            self.inference.table = table;
            if ty.has_projection() {
                complete = false;
            } else {
                self.inference.constrain_infer_tys(destination, &ty);
            }
        }
        state.return_projection_complete = complete;
        Ok(())
    }
}
