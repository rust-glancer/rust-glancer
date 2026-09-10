use std::sync::Arc;

use rg_ir_model::{BindingId, ExprId, StmtId};

use rg_ty::{
    GenericArg, GenericArgs, Ty,
    inference::{InferenceTable, UnknownTypeInstantiationBuilder},
};

use crate::{BodyData, BodyFacts, CallFacts, body::BodyQueryView};

use super::{call::CallInferenceState, facts::InferenceFacts};

/// Mutable body-owned inference state used before facts cross the persistence boundary.
///
/// `BodyFacts` stores ordinary finalized `Ty` values. This context instead keeps one inference
/// table plus dense expression and binding slots that may refer into it, allowing evidence from
/// annotations, calls, patterns, and trait selection to describe the same unknown type. Call and
/// written-annotation state lives here for the same reason: revisiting a fixed-point rule must
/// refine its existing variables rather than allocate unrelated replacements.
///
/// The structural `BodyData` is never copied into this state. `finish` resolves the live slots and
/// writes only their durable conclusions into the already-aligned `BodyFacts` sidecar.
#[derive(Clone)]
pub(crate) struct BodyInferenceCtx {
    pub(super) table: InferenceTable,
    // Trait-obligation probes clone this context transactionally, but they do not edit call-site
    // setup. Copy-on-write keeps those probes from cloning every canonical call projection.
    call_inference: Arc<Vec<Option<CallInferenceState>>>,
    expr_tys: InferenceFacts<ExprId>,
    binding_tys: InferenceFacts<BindingId>,
    // Written annotation holes are body-owned inference positions too. Keep their types stable so
    // the outer fixed point can reapply annotation relationships without allocating a new `_`
    // variable on every pass.
    statement_expected_tys: Arc<Vec<Option<Ty>>>,
}

/// Copy-on-write read snapshot shared by one inference transfer step.
///
/// Cloning the fact tables only increments their `Arc`s. A mutation in the inference context then
/// detaches the changed table once, so queries see one coherent step-start view without keeping a
/// parallel mutable fact lane. The outer fixed point creates the next snapshot from the refined
/// live facts.
#[derive(Clone)]
pub(crate) struct BodyInferenceSnapshot {
    expr_tys: InferenceFacts<ExprId>,
    binding_tys: InferenceFacts<BindingId>,
}

/// Canonical convergence key captured at one fixed-point boundary.
///
/// Raw inference variable ids are allocation details, so comparing the live tables directly would
/// report progress when a retried rule merely replaces `?14` with `?15`. The flattened facts keep
/// relationships between slots but compare them modulo those ids. Finalized selected calls are
/// included because a newly selected target or substitution is semantic progress too.
pub(crate) struct BodyInferenceProgress {
    calls: Vec<(ExprId, CallFacts)>,
    inference_facts: GenericArgs,
}

impl PartialEq for BodyInferenceProgress {
    fn eq(&self, other: &Self) -> bool {
        self.calls == other.calls
            && self
                .inference_facts
                .equivalent_modulo_inference_ids(&other.inference_facts)
    }
}

impl Eq for BodyInferenceProgress {}

impl BodyInferenceCtx {
    pub(crate) fn new(expr_count: usize, binding_count: usize, statement_count: usize) -> Self {
        Self {
            table: InferenceTable::new(),
            call_inference: Arc::new(vec![None; expr_count]),
            expr_tys: InferenceFacts::new(expr_count),
            binding_tys: InferenceFacts::new(binding_count),
            statement_expected_tys: Arc::new(vec![None; statement_count]),
        }
    }

    /// Expose accumulated resolutions together with the latest live type slots to queries.
    pub(crate) fn view<'a>(
        &'a self,
        body: &'a BodyData,
        resolutions: &'a BodyFacts,
    ) -> BodyQueryView<'a> {
        BodyQueryView::for_inference(
            body,
            resolutions,
            self.expr_tys.as_slice(),
            self.binding_tys.as_slice(),
        )
    }

    /// Freeze a cheap read view for one inference transfer step.
    pub(crate) fn snapshot(&self) -> BodyInferenceSnapshot {
        BodyInferenceSnapshot {
            expr_tys: self.expr_tys.clone(),
            binding_tys: self.binding_tys.clone(),
        }
    }

    pub(crate) fn table(&self) -> &InferenceTable {
        &self.table
    }

    pub(crate) fn table_mut(&mut self) -> &mut InferenceTable {
        &mut self.table
    }

    pub(super) fn call_inference(&self, call: ExprId) -> Option<CallInferenceState> {
        self.call_inference[call.0].clone()
    }

    /// Return a target that this body's call inference has already selected uniquely.
    pub(crate) fn selected_call_function(&self, call: ExprId) -> Option<rg_ir_model::FunctionRef> {
        self.call_inference[call.0]
            .as_ref()
            .map(CallInferenceState::function)
    }

    pub(super) fn set_call_inference(&mut self, call: ExprId, call_inference: CallInferenceState) {
        if self.call_inference[call.0].as_ref() == Some(&call_inference) {
            return;
        }
        Arc::make_mut(&mut self.call_inference)[call.0] = Some(call_inference);
    }

    pub(crate) fn set_expr_ty(&mut self, expr: ExprId, ty: &Ty) {
        let current = self.expr_tys.get(expr);
        if current.has_var() {
            self.table.unify(&current, ty);
            return;
        }
        self.set_expr_fact(expr, ty.clone());
    }

    pub(crate) fn set_expr_infer_ty(&mut self, expr: ExprId, ty: Ty) -> bool {
        let previous_ty = self.expr_tys.get(expr);
        let changed = self.table.unify(&previous_ty, &ty);
        if previous_ty.has_var() {
            return changed;
        }
        self.refine_expr_fact(expr, ty) || changed
    }

    /// Commit a semantic normalization of the expression's existing type shape.
    ///
    /// Ordinary evidence is monotonic: a different outer shape must not overwrite an established
    /// fact. Associated-type normalization is different because `Trait::Item<Self>` and its
    /// selected concrete value are two representations of the same type. Replace a projection
    /// shape after normalization, while still unifying through any live expected-type slot.
    pub(crate) fn set_expr_normalized_ty(&mut self, expr: ExprId, ty: Ty) -> bool {
        let previous_ty = self.expr_tys.get(expr);
        // A root slot should absorb the evidence. A projection may itself contain live generic or
        // closure slots, but its outer alias still has to be replaced by the proven normal form.
        if matches!(previous_ty, Ty::InferVar { .. }) {
            return self.table.unify(&previous_ty, &ty);
        }
        if previous_ty.has_projection() {
            return self.set_expr_fact(expr, ty);
        }
        self.refine_expr_fact(expr, ty)
    }

    pub(crate) fn expr_ty(&self, expr: ExprId) -> Ty {
        self.expr_tys.get(expr)
    }

    pub(crate) fn expr_ty_ref(&self, expr: ExprId) -> &Ty {
        self.expr_tys.get_ref(expr)
    }

    #[cfg(test)]
    pub(crate) fn binding_ty(&self, binding: BindingId) -> Ty {
        self.binding_tys.get(binding)
    }

    pub(crate) fn root_resolved_expr_ty(&self, expr: ExprId) -> Ty {
        self.expr_tys.root_resolved(&self.table, expr)
    }

    pub(crate) fn root_resolved_ty(&self, ty: &Ty) -> Ty {
        self.table.resolve_root_var(ty)
    }

    /// Instantiate unknowns nested inside a selected call return shape.
    pub(crate) fn instantiate_expr_nested_unknown_ty(&mut self, expr: ExprId, ty: &Ty) -> bool {
        if matches!(ty, Ty::Unknown) {
            return false;
        }

        // A fixed-point revisit often presents the same weak producer shape again, for example
        // `Vec<unknown>` after this expression already owns `Vec<?T>`. The existing structure can
        // absorb any new concrete evidence directly; allocating another `?T` would only grow an
        // alias chain that is invisible to convergence.
        let existing_ty = self.root_resolved_expr_ty(expr);
        if !matches!(existing_ty, Ty::Unknown | Ty::InferVar { .. }) && !existing_ty.has_unknown() {
            return self.set_expr_infer_ty(expr, ty.clone());
        }

        let (infer_ty, used_vars) = {
            let mut builder = UnknownTypeInstantiationBuilder::new(&mut self.table);
            let infer_ty = builder.ty_from_ty(ty);
            (infer_ty, builder.used_type_vars())
        };

        if !used_vars {
            return false;
        }

        // A partially known fact may contain both live variables and raw `Unknown` children.
        // Unification links its existing variables, while refinement installs slots for the raw
        // children so the next pass sees a complete stable structure.
        self.set_expr_infer_ty(expr, infer_ty.clone());
        self.refine_expr_fact(expr, infer_ty);
        true
    }

    pub(crate) fn set_binding_ty(&mut self, binding: BindingId, ty: &Ty) {
        let current = self.binding_tys.get(binding);
        if current.has_var() {
            self.table.unify(&current, ty);
            return;
        }
        self.set_binding_fact(binding, ty.clone());
    }

    /// Set a binding to an inference-aware type, preserving any previous evidence.
    pub(crate) fn set_binding_infer_ty(&mut self, binding: BindingId, ty: Ty) -> bool {
        let previous_ty = self.binding_tys.get(binding);
        let changed = self.table.unify(&previous_ty, &ty);
        // An unannotated pattern can first see an unresolved call projection and then its concrete
        // normal form on the next fixed-point pass. The latter is stronger evidence even when the
        // old projection contains nested inference slots that ordinary unification cannot cross.
        if previous_ty.has_projection() && !ty.has_projection() {
            return self.set_binding_fact(binding, ty) || changed;
        }
        if previous_ty.has_var() {
            return changed;
        }
        self.refine_binding_fact(binding, ty) || changed
    }

    /// Copy a binding slot into a path expression that reads it.
    pub(crate) fn set_expr_from_binding(&mut self, expr: ExprId, binding: BindingId) -> bool {
        let binding_ty = self.binding_tys.get(binding);
        let expr_ty = self.expr_tys.get(expr);
        // A local path and its binding are one equality relationship, not a one-way copy. The
        // expression can already carry expected-type evidence before the binding initializer has
        // reached it through the outer fixed point. Preserve both facts and fill whichever side
        // is still weak; live inference slots are linked by unification.
        let unified = self.table.unify(&expr_ty, &binding_ty);
        let expr_changed = self.refine_expr_fact(expr, binding_ty);
        let binding_changed = self.refine_binding_fact(binding, expr_ty);
        unified || expr_changed || binding_changed
    }

    pub(crate) fn constrain_expr_ty(&mut self, expr: ExprId, expected_ty: &Ty) -> bool {
        // A diverging expression can inhabit every expected value type, but its own type remains
        // `!`. Treating this as equality would solve a destination slot to `!`; later evidence for
        // the real destination type would then conflict instead of refining that slot.
        if matches!(self.root_resolved_expr_ty(expr), Ty::Never)
            && !matches!(self.table.resolve_root_var(expected_ty), Ty::Never)
        {
            return false;
        }

        // `Unknown` means that no producer fact has arrived yet. Expected types are still real
        // evidence, so retain their shape now; a later producer will unify with or refine it.
        self.set_expr_infer_ty(expr, expected_ty.clone())
    }

    pub(crate) fn constrain_infer_tys(&mut self, lhs: &Ty, rhs: &Ty) -> bool {
        self.table.unify(lhs, rhs)
    }

    pub(crate) fn set_statement_expected_ty(&mut self, statement: StmtId, ty: Ty) {
        Arc::make_mut(&mut self.statement_expected_tys)[statement.0] = Some(ty);
    }

    pub(crate) fn statement_expected_ty(&self, statement: StmtId) -> Option<Ty> {
        self.statement_expected_tys[statement.0].clone()
    }

    /// Capture an alpha-equivalent key for fixed-point convergence.
    pub(crate) fn progress(&self) -> BodyInferenceProgress {
        // Inference IDs are allocation details, but their relationships are semantic. Flatten all
        // body slots into one argument list so alpha-equivalence preserves sharing such as
        // `binding: ?T` and `path: ?T` while treating a retried `?14` / `?15` as the same state.
        let inference_facts = self
            .expr_tys
            .as_slice()
            .iter()
            .chain(self.binding_tys.as_slice())
            .chain(self.statement_expected_tys.iter().flatten())
            .map(|ty| GenericArg::Type(Box::new(self.table.canonicalize(ty))))
            .collect::<GenericArgs>();
        BodyInferenceProgress {
            calls: self.finalize_calls(true),
            inference_facts,
        }
    }

    pub(crate) fn has_progressed_since(&self, before: &BodyInferenceProgress) -> bool {
        self.progress() != *before
    }

    /// Consume live inference state into the persisted body sidecar.
    ///
    /// This is the only boundary that writes expression and binding types into `BodyFacts`.
    /// After convergence, unsolved numeric variables receive their language defaults. An
    /// incomplete fixed point instead keeps every unresolved slot unknown because later evidence
    /// could still choose a non-default numeric type. Selected calls retain only finalized
    /// full-arity arguments under the same policy.
    pub(crate) fn finish(self, mut facts: BodyFacts, inference_complete: bool) -> BodyFacts {
        debug_assert_eq!(facts.exprs.len(), self.expr_tys.as_slice().len());
        debug_assert_eq!(facts.bindings.len(), self.binding_tys.as_slice().len());

        for expr_idx in 0..self.expr_tys.as_slice().len() {
            let expr = ExprId(expr_idx);
            let ty = self.finalize_ty(self.expr_tys.get_ref(expr), inference_complete);
            facts.set_expr_ty(expr, ty);
        }
        for binding_idx in 0..self.binding_tys.as_slice().len() {
            let binding = BindingId(binding_idx);
            let ty = self.finalize_ty(self.binding_tys.get_ref(binding), inference_complete);
            facts.set_binding_ty(binding, ty);
        }
        facts.set_calls(self.finalize_calls(inference_complete));
        facts
    }

    #[cfg(test)]
    pub(crate) fn finalize_expr_ty(&self, expr: ExprId) -> Ty {
        self.expr_tys.finalize(&self.table, expr)
    }

    #[cfg(test)]
    pub(crate) fn finalize_binding_ty(&self, binding: BindingId) -> Ty {
        self.binding_tys.finalize(&self.table, binding)
    }

    /// Finalize only expressions for which call lookup selected one semantic function.
    fn finalize_calls(&self, inference_complete: bool) -> Vec<(ExprId, CallFacts)> {
        self.call_inference
            .iter()
            .enumerate()
            .filter_map(|(index, state)| {
                state.as_ref().map(|state| {
                    (
                        ExprId(index),
                        state.finalize(&self.table, inference_complete),
                    )
                })
            })
            .collect()
    }

    /// Erase live variables under the policy chosen by the outer fixed-point boundary.
    fn finalize_ty(&self, ty: &Ty, inference_complete: bool) -> Ty {
        if inference_complete {
            self.table.finalize(ty)
        } else {
            self.table.finalize_without_numeric_defaults(ty)
        }
    }

    /// Compare body-side facts canonically while preserving live inference slots.
    fn set_expr_fact(&mut self, expr: ExprId, ty: Ty) -> bool {
        self.expr_tys.set(&self.table, expr, ty)
    }

    /// Merge another observation without replacing stronger expression evidence.
    fn refine_expr_fact(&mut self, expr: ExprId, ty: Ty) -> bool {
        self.expr_tys.refine(&self.table, expr, ty)
    }

    /// Store a new slot even if its current weak evidence still canonicalizes to the old shape.
    pub(crate) fn set_expr_fact_allowing_weak_slot(&mut self, expr: ExprId, ty: Ty) -> bool {
        self.expr_tys.set_allowing_weak_slot(&self.table, expr, ty)
    }

    /// Compare binding-side facts canonically while preserving live inference slots.
    fn set_binding_fact(&mut self, binding: BindingId, ty: Ty) -> bool {
        self.binding_tys.set(&self.table, binding, ty)
    }

    /// Merge another observation without replacing stronger binding evidence.
    fn refine_binding_fact(&mut self, binding: BindingId, ty: Ty) -> bool {
        self.binding_tys.refine(&self.table, binding, ty)
    }
}

impl BodyInferenceSnapshot {
    /// Build the query view read by one transfer step.
    ///
    /// Resolutions come from the pass-owned sidecar because name resolution is not part of the
    /// inference table. Types come from this snapshot so every query in the step sees the same
    /// starting state while the live context is being refined.
    pub(crate) fn view<'a>(
        &'a self,
        body: &'a BodyData,
        resolutions: &'a BodyFacts,
    ) -> BodyQueryView<'a> {
        BodyQueryView::for_inference(
            body,
            resolutions,
            self.expr_tys.as_slice(),
            self.binding_tys.as_slice(),
        )
    }
}
