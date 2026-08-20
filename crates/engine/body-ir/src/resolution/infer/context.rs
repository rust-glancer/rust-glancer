use std::sync::Arc;

use rg_ir_model::{BindingId, BodyRef, ExprBinaryOp, ExprId, ExprUnaryOp, StmtId};

use rg_ty::{
    ClosureTyId, GenericArg, GenericArgs, PrimitiveTy, Ty,
    inference::InferVarKind,
    inference::{InferenceTable, UnknownTypeInstantiationBuilder},
    ty_for_binary, ty_for_unary,
};

use crate::{
    BodyData, BodyFacts, CallFacts,
    ir::{BodyQueryView, ExprWrapperKind},
};

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

    /// Give one closure a stable identity plus live slots for every parameter and its return.
    ///
    /// An expected `Fn(User) -> Name` bound and evidence from the closure body must constrain the
    /// same slots. They are allocated once during inference initialization and then carried inside
    /// the closure's semantic type through every fixed-point pass.
    pub(crate) fn set_expr_closure_ty(
        &mut self,
        body_ref: BodyRef,
        expr: ExprId,
        param_count: usize,
    ) -> bool {
        let params = (0..param_count)
            .map(|_| self.table.new_type_var())
            .collect();
        let ret = self.table.new_type_var();
        self.set_expr_infer_ty(
            expr,
            Ty::closure(ClosureTyId::new(body_ref, expr), params, ret),
        )
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

    pub(crate) fn set_expr_integer_var(&mut self, expr: ExprId) {
        if !matches!(self.expr_tys.get_ref(expr), Ty::Unknown) {
            return;
        }
        let ty = self.table.new_integer_var();
        self.set_expr_infer_ty(expr, ty);
    }

    pub(crate) fn set_expr_float_var(&mut self, expr: ExprId) {
        if !matches!(self.expr_tys.get_ref(expr), Ty::Unknown) {
            return;
        }
        let ty = self.table.new_float_var();
        self.set_expr_infer_ty(expr, ty);
    }

    pub(crate) fn set_expr_tuple_from_fields(&mut self, expr: ExprId, fields: &[ExprId]) {
        // Tuple expressions and their fields are one equality relationship. The tuple may already
        // contain live slots introduced by an expected type or closure output before a child call
        // resolves. Re-link every field on each pass so that later child evidence solves those
        // existing slots instead of replacing the tuple shape.
        let field_tys = fields
            .iter()
            .map(|field| self.expr_ty(*field))
            .collect::<Vec<_>>();
        let tuple_ty = Ty::tuple(field_tys);
        let previous_ty = self.expr_tys.get(expr);
        self.table.unify(&previous_ty, &tuple_ty);
        self.set_expr_fact_allowing_weak_slot(expr, tuple_ty);
    }

    pub(crate) fn set_expr_array_from_elements(
        &mut self,
        expr: ExprId,
        elements: &[ExprId],
        len: Option<String>,
    ) {
        if elements.is_empty() {
            self.set_expr_infer_ty(expr, Ty::Unknown);
            return;
        }

        // Array elements share one element type. Link every element slot through that type so
        // sibling evidence and expected array types can solve literals and generic call results.
        // The fixed point may revisit the same array, so keep the old inference slot when the
        // shape matches.
        let len = rg_ty::ConstValue::from(len);
        let element_ty = match self.expr_tys.get_ref(expr) {
            Ty::Array {
                inner,
                len: existing_len,
            } if existing_len == &len && Self::is_inference_owned_slot(inner) => {
                inner.as_ref().clone()
            }
            _ => self.table.new_type_var(),
        };
        for element in elements {
            let evidence = self.expr_tys.get(*element);
            self.table.unify(&element_ty, &evidence);
        }

        self.set_expr_fact_allowing_weak_slot(
            expr,
            Ty::Array {
                inner: Box::new(element_ty),
                len,
            },
        );
    }

    pub(crate) fn set_expr_repeat_array_from_initializer(
        &mut self,
        expr: ExprId,
        initializer: Option<ExprId>,
        len: Option<String>,
    ) {
        let Some(initializer) = initializer else {
            self.set_expr_infer_ty(expr, Ty::Unknown);
            return;
        };

        self.set_expr_fact_allowing_weak_slot(
            expr,
            Ty::Array {
                inner: Box::new(self.expr_tys.get(initializer)),
                len: len.into(),
            },
        );
    }

    pub(crate) fn set_expr_wrapper_from_inner(
        &mut self,
        expr: ExprId,
        kind: ExprWrapperKind,
        inner: Option<ExprId>,
        fallback_ty: &Ty,
    ) {
        let Some(inner) = inner else {
            self.set_expr_ty(expr, fallback_ty);
            return;
        };
        let inner_ty = self.expr_tys.get(inner);

        let ty = match kind {
            ExprWrapperKind::Paren | ExprWrapperKind::Await => inner_ty,
            ExprWrapperKind::Ref { mutability } => Ty::reference(mutability, inner_ty),
            ExprWrapperKind::Try | ExprWrapperKind::Return => fallback_ty.clone(),
        };
        self.set_expr_fact_allowing_weak_slot(expr, ty);
    }

    /// Apply a primitive unary relationship without defaulting a numeric literal prematurely.
    pub(crate) fn set_expr_unary_from_inner(
        &mut self,
        expr: ExprId,
        op: ExprUnaryOp,
        inner: ExprId,
    ) {
        let inner_ty = self.expr_tys.get(inner);
        let resolved = self.table.resolve_root_var(&inner_ty);
        let ty = match (&resolved, op) {
            (
                Ty::InferVar {
                    kind: InferVarKind::Integer,
                    ..
                },
                ExprUnaryOp::Not | ExprUnaryOp::Neg,
            )
            | (
                Ty::InferVar {
                    kind: InferVarKind::Float,
                    ..
                },
                ExprUnaryOp::Neg,
            ) => inner_ty,
            _ => ty_for_unary(op, &resolved),
        };
        self.set_expr_fact_allowing_weak_slot(expr, ty);
    }

    /// Apply one primitive binary relationship to the live operand slots.
    ///
    /// Numeric literals stay as numeric variables until another operand or an expected type
    /// chooses their concrete primitive. The result then shares that evidence instead of reading
    /// a separate defaulted `Ty` lane.
    pub(crate) fn set_expr_binary_from_operands(
        &mut self,
        expr: ExprId,
        op: ExprBinaryOp,
        lhs: ExprId,
        rhs: ExprId,
    ) {
        if op.is_logical() || op.is_comparison() {
            self.set_expr_infer_ty(expr, Ty::Primitive(PrimitiveTy::Bool));
            return;
        }

        let lhs_ty = self.expr_tys.get(lhs);
        let rhs_ty = self.expr_tys.get(rhs);
        let lhs_resolved = self.table.resolve_root_var(&lhs_ty);
        let rhs_resolved = self.table.resolve_root_var(&rhs_ty);

        let ty = if matches!(op, ExprBinaryOp::Shl | ExprBinaryOp::Shr) {
            let rhs_is_integral = match &rhs_resolved {
                Ty::Unknown
                | Ty::InferVar {
                    kind: InferVarKind::Integer,
                    ..
                } => true,
                Ty::Primitive(primitive) => primitive.is_integral(),
                _ => false,
            };
            match (&lhs_resolved, rhs_is_integral) {
                (Ty::Primitive(primitive), true) if primitive.is_integral() => lhs_resolved,
                (
                    Ty::InferVar {
                        kind: InferVarKind::Integer,
                        ..
                    },
                    true,
                ) => lhs_ty,
                _ => ty_for_binary(op, &lhs_resolved, &rhs_resolved),
            }
        } else {
            let accepts = |ty: &Ty| match (op, ty) {
                (
                    ExprBinaryOp::Add
                    | ExprBinaryOp::Sub
                    | ExprBinaryOp::Mul
                    | ExprBinaryOp::Div
                    | ExprBinaryOp::Rem,
                    Ty::Primitive(primitive),
                ) => primitive.is_numeric(),
                (
                    ExprBinaryOp::BitAnd | ExprBinaryOp::BitOr | ExprBinaryOp::BitXor,
                    Ty::Primitive(primitive),
                ) => primitive.is_integral() || primitive.is_bool(),
                (
                    ExprBinaryOp::Add
                    | ExprBinaryOp::Sub
                    | ExprBinaryOp::Mul
                    | ExprBinaryOp::Div
                    | ExprBinaryOp::Rem,
                    Ty::InferVar { kind, .. },
                ) => matches!(kind, InferVarKind::Integer | InferVarKind::Float),
                (
                    ExprBinaryOp::BitAnd | ExprBinaryOp::BitOr | ExprBinaryOp::BitXor,
                    Ty::InferVar { kind, .. },
                ) => matches!(kind, InferVarKind::Integer),
                _ => false,
            };

            if accepts(&lhs_resolved) && accepts(&rhs_resolved) {
                self.table.unify(&lhs_ty, &rhs_ty);
                let resolved = self.table.resolve_root_var(&lhs_ty);
                if matches!(resolved, Ty::InferVar { .. }) {
                    lhs_ty
                } else {
                    ty_for_binary(op, &resolved, &resolved)
                }
            } else {
                ty_for_binary(op, &lhs_resolved, &rhs_resolved)
            }
        };

        self.set_expr_fact_allowing_weak_slot(expr, ty);
    }

    pub(crate) fn set_expr_block_from_tail(&mut self, expr: ExprId, tail: Option<ExprId>) {
        let ty = tail.map(|tail| self.expr_tys.get(tail)).unwrap_or(Ty::Unit);
        self.set_expr_fact_allowing_weak_slot(expr, ty);
    }

    pub(crate) fn set_expr_if_from_branches(
        &mut self,
        expr: ExprId,
        then_branch: Option<ExprId>,
        else_branch: Option<ExprId>,
    ) {
        let Some(else_branch) = else_branch else {
            self.set_expr_infer_ty(expr, Ty::Unit);
            return;
        };

        self.set_expr_common_result_from_exprs(expr, then_branch.into_iter().chain([else_branch]));
    }

    pub(crate) fn set_expr_match_from_arms(
        &mut self,
        expr: ExprId,
        arms: impl Iterator<Item = ExprId>,
    ) {
        self.set_expr_common_result_from_exprs(expr, arms);
    }

    fn set_expr_common_result_from_exprs(
        &mut self,
        expr: ExprId,
        result_exprs: impl Iterator<Item = ExprId>,
    ) {
        // Branch-like expressions need one shared result slot. Diverging branches have type `!`,
        // but they do not produce a value that should conflict with the other branches.
        // The fixed point may revisit the same branch expression, so reuse only an existing
        // inference slot.
        let result_ty = match self.expr_tys.get_ref(expr) {
            Ty::Unknown | Ty::Never => self.table.new_type_var(),
            ty if Self::is_inference_owned_slot(ty) => ty.clone(),
            _ => self.table.new_type_var(),
        };
        let mut has_result = false;
        let mut has_value_result = false;
        for result_expr in result_exprs {
            has_result = true;
            let branch_ty = self.expr_tys.root_resolved(&self.table, result_expr);
            if matches!(branch_ty, Ty::Never) {
                continue;
            }

            has_value_result = true;
            if matches!(branch_ty, Ty::Unknown) {
                continue;
            }
            // A branch may read the value being assigned by the whole expression, e.g.
            // `x = match state { Keep => x, Change => next }`. Use the root-resolved branch
            // type so already-detected cycles stay as `Unknown` instead of recursing again.
            self.table.unify(&result_ty, &branch_ty);
        }

        let ty = if has_value_result {
            result_ty
        } else if has_result {
            Ty::Never
        } else {
            // Note that we don't handle "empty blocks" but "lack of blocks" here,
            // "empty blocks" are handled separately -- these are real exprs that resolve to unit,
            // while here we are dealing with incomplete code like `match` with no arms.
            Ty::Unknown
        };
        self.set_expr_fact_allowing_weak_slot(expr, ty);
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

    /// Return whether a fact still points into the inference table.
    fn is_inference_owned_slot(ty: &Ty) -> bool {
        ty.has_var()
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
    fn set_expr_fact_allowing_weak_slot(&mut self, expr: ExprId, ty: Ty) -> bool {
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
