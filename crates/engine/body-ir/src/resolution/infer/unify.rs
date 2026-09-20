//! Connect body expressions and bindings through the shared inference table.
//!
//! The table knows about type variables and equality. This layer gives those relationships body
//! identities: a binding, a path that reads it, and its initializer can all refer to the same slot.
//! Updating that slot then reaches every use without replacing each expression's stored type.

use rg_arena::Arena;
use rg_ir_model::{BindingId, BodyRef, ExprBinaryOp, ExprId, ExprUnaryOp};

use rg_ty::{
    ClosureTyId, PrimitiveTy, Ty,
    inference::{InferVarKind, InferenceTable},
    ty_for_binary, ty_for_unary,
};

use crate::{BodyFacts, ExprFacts, body::facts::BodyResolution};

use super::call::CallInferenceState;

/// Live expression and binding facts, selected calls, and their unification table.
/// Finalization resolves the type variables in these arenas before moving them into `BodyFacts`.
///
/// A stored `Ty` can contain inference variables. Cloning it preserves their ids in this table,
/// so expressions can share a type relationship without copying its latest resolved value.
pub(crate) struct InferenceState {
    pub(super) table: InferenceTable,
    call_inference: Vec<Option<CallInferenceState>>,
    exprs: Arena<ExprId, ExprFacts>,
    binding_tys: Arena<BindingId, Ty>,
}

impl InferenceState {
    pub(crate) fn new(expr_count: usize, binding_count: usize) -> Self {
        Self {
            table: InferenceTable::new(),
            call_inference: (0..expr_count).map(|_| None).collect(),
            exprs: Arena::from_vec(vec![ExprFacts::default(); expr_count]),
            binding_tys: Arena::from_vec(vec![Ty::Unknown; binding_count]),
        }
    }

    pub(crate) fn table(&self) -> &InferenceTable {
        &self.table
    }

    pub(crate) fn table_mut(&mut self) -> &mut InferenceTable {
        &mut self.table
    }

    pub(super) fn take_call_inference(&mut self, call: ExprId) -> Option<CallInferenceState> {
        self.call_inference[call.0].take()
    }

    /// Return a target that this body's call inference has already selected uniquely.
    pub(crate) fn selected_call_function(&self, call: ExprId) -> Option<rg_ir_model::FunctionRef> {
        self.call_inference[call.0]
            .as_ref()
            .map(CallInferenceState::function)
    }

    pub(super) fn set_call_inference(&mut self, call: ExprId, call_inference: CallInferenceState) {
        self.call_inference[call.0] = Some(call_inference);
    }

    pub(crate) fn call_is_complete(&self, call: ExprId) -> bool {
        self.call_inference[call.0]
            .as_ref()
            .is_some_and(CallInferenceState::is_complete)
    }

    pub(crate) fn call_needs_fulfillment(&self, call: ExprId) -> bool {
        self.call_inference[call.0]
            .as_ref()
            .is_some_and(|state| state.needs_fulfillment(&self.table))
    }

    pub(super) fn call_result_is_pending(&self, call: ExprId, ty: &Ty) -> bool {
        match &self.call_inference[call.0] {
            Some(state) => state.result_is_pending(&self.table, ty),
            None => self.root_resolved_expr_ty(call) == *ty,
        }
    }

    pub(crate) fn call_input(&self, call: ExprId) -> Ty {
        self.call_inference[call.0]
            .as_ref()
            .map(CallInferenceState::input)
            .unwrap_or(Ty::Unknown)
    }

    /// Store a type supplied by syntax or lookup. If the expression already carries live slots,
    /// constrain them instead of replacing the type that its consumers share.
    pub(crate) fn set_expr_ty(&mut self, expr: ExprId, ty: Ty) {
        let current = self.exprs[expr].ty.clone();
        if current.has_var() {
            self.table.unify(&current, &ty);
            return;
        }
        self.set_expr_fact(expr, ty);
    }

    pub(crate) fn set_expr_facts(&mut self, expr: ExprId, resolution: BodyResolution, ty: Ty) {
        self.set_expr_ty(expr, ty);
        self.exprs[expr].resolution = resolution;
    }

    pub(crate) fn expr_resolution(&self, expr: ExprId) -> &BodyResolution {
        &self.exprs[expr].resolution
    }

    pub(crate) fn set_expr_resolution(&mut self, expr: ExprId, resolution: BodyResolution) {
        self.exprs[expr].resolution = resolution;
    }

    /// Add evidence from an expectation or a deferred result. It may know only part of the type,
    /// so link its variables and fill missing shape without discarding stronger stored facts.
    pub(crate) fn set_expr_infer_ty(&mut self, expr: ExprId, ty: Ty) {
        let previous_ty = self.exprs[expr].ty.clone();
        self.table.unify(&previous_ty, &ty);
        if previous_ty.has_var() {
            return;
        }
        self.refine_expr_fact(expr, ty)
    }

    /// Retain the call's substituted return after binding its arguments.
    ///
    /// A root slot must stay linked to its consumers. Otherwise merge the return shape with the
    /// stored result, preserving its nested slots and any stronger evidence already present.
    pub(crate) fn set_call_return_ty(&mut self, expr: ExprId, ty: Ty) {
        let previous_ty = self.exprs[expr].ty.clone();
        if matches!(previous_ty, Ty::InferVar { .. }) {
            return self.table.unify(&previous_ty, &ty);
        }
        self.refine_expr_fact(expr, ty)
    }

    /// Keep consumers connected while a supported producer waits for type evidence.
    ///
    /// For `[make(); 3]`, retaining the initializer as `?T` gives the array type `[?T; 3]`.
    /// A later array annotation can then constrain `make()` too. Copying `Unknown` into the array
    /// would lose that relationship. An expression with an existing type already has what we need.
    pub(crate) fn expr_slot(&mut self, expr: ExprId) -> Ty {
        let ty = self.expr_ty(expr);
        if !matches!(ty, Ty::Unknown) {
            return ty;
        }
        let ty = self.table.new_type_var();
        self.set_expr_fact_allowing_weak_slot(expr, ty.clone());
        ty
    }

    /// Reads of an unresolved binding need one shared slot. Seed it with any partial type already
    /// known for the binding, so later evidence from a path can refine that same type.
    pub(crate) fn binding_slot(&mut self, binding: BindingId) -> Ty {
        let previous = self.binding_tys[binding].clone();
        if !previous.has_unknown() {
            return previous;
        }
        let ty = self.table.new_type_var();
        self.table.unify(&ty, &previous);
        Self::set_weak_fact(&self.table, &mut self.binding_tys[binding], ty.clone());
        ty
    }

    pub(crate) fn expr_ty(&self, expr: ExprId) -> Ty {
        self.exprs[expr].ty.clone()
    }

    pub(crate) fn binding_ty(&self, binding: BindingId) -> Ty {
        self.binding_tys[binding].clone()
    }

    pub(crate) fn root_resolved_expr_ty(&self, expr: ExprId) -> Ty {
        self.table.resolve_root_var(&self.exprs[expr].ty)
    }

    pub(crate) fn root_resolved_ty(&self, ty: &Ty) -> Ty {
        self.table.resolve_root_var(ty)
    }

    /// Give unknown children of a known producer shape their own slots, as in `Vec<unknown>`.
    /// Calls and constructors can then learn omitted type arguments from their uses.
    pub(crate) fn instantiate_expr_nested_unknown_ty(&mut self, expr: ExprId, ty: &Ty) {
        if matches!(ty, Ty::Unknown) {
            return;
        }

        // A producer and an expectation can describe the same weak shape, for example
        // `Vec<unknown>` and `Vec<?T>`. Preserve the established slot when connecting them.
        let existing_ty = self.root_resolved_expr_ty(expr);
        if !matches!(existing_ty, Ty::Unknown | Ty::InferVar { .. }) && !existing_ty.has_unknown() {
            return self.set_expr_infer_ty(expr, ty.clone());
        }

        if !ty.has_unknown() {
            return;
        }
        let infer_ty = self.table.instantiate_nested_unknowns(ty);

        // A partially known fact may contain both live variables and raw `Unknown` children.
        // Unification links its existing variables, while refinement installs slots for the raw
        // children so every consumer sees the same stable structure.
        self.set_expr_infer_ty(expr, infer_ty.clone());
        self.refine_expr_fact(expr, infer_ty);
    }

    pub(crate) fn set_binding_ty(&mut self, binding: BindingId, ty: &Ty) {
        let current = self.binding_tys[binding].clone();
        if current.has_var() {
            self.table.unify(&current, ty);
            return;
        }
        self.set_binding_fact(binding, ty.clone());
    }

    /// Set a binding to an inference-aware type, preserving any previous evidence.
    pub(crate) fn set_binding_infer_ty(&mut self, binding: BindingId, ty: Ty) {
        let previous_ty = self.binding_tys[binding].clone();
        self.table.unify(&previous_ty, &ty);
        // A normalized associated type is stronger evidence than its unresolved spelling.
        // Ordinary structural unification cannot relate these two representations.
        if previous_ty.has_projection() && !ty.has_projection() {
            return self.set_binding_fact(binding, ty);
        }
        if previous_ty.has_var() {
            return;
        }
        self.refine_binding_fact(binding, ty)
    }

    /// Copy a binding slot into a path expression that reads it.
    pub(crate) fn set_expr_from_binding(&mut self, expr: ExprId, binding: BindingId) {
        let binding_ty = self.binding_slot(binding);
        let expr_ty = self.exprs[expr].ty.clone();
        // A local path and its binding are one equality relationship, not a one-way copy. The
        // expression can already carry an expectation while its initializer is still waiting for
        // a selected call or projection. Preserve both facts and fill whichever side
        // is still weak; live inference slots are linked by unification.
        self.table.unify(&expr_ty, &binding_ty);
        self.set_expr_fact_allowing_weak_slot(expr, binding_ty);
        self.refine_binding_fact(binding, expr_ty);
    }

    pub(crate) fn constrain_expr_ty(&mut self, expr: ExprId, expected_ty: &Ty) {
        // A diverging expression can inhabit every expected value type, but its own type remains
        // `!`. Treating this as equality would solve a destination slot to `!`; later evidence for
        // the real destination type would then conflict instead of refining that slot.
        if matches!(self.root_resolved_expr_ty(expr), Ty::Never)
            && !matches!(self.table.resolve_root_var(expected_ty), Ty::Never)
        {
            return;
        }

        // `Unknown` means that no producer fact has arrived yet. Expected types are still real
        // evidence, so retain their shape now; a later producer will unify with or refine it.
        self.set_expr_infer_ty(expr, expected_ty.clone())
    }

    pub(crate) fn constrain_infer_tys(&mut self, lhs: &Ty, rhs: &Ty) {
        self.table.unify(lhs, rhs)
    }

    /// Consume live inference state into the persisted body sidecar.
    ///
    /// The expression and binding arenas keep their ids and storage across this boundary. Only
    /// their types change: readers must never receive variables owned by the inference table.
    pub(crate) fn finish(self) -> BodyFacts {
        let Self {
            mut table,
            call_inference,
            mut exprs,
            mut binding_tys,
        } = self;

        // Unresolved return projections fall back to their associated-type spelling. Settle those
        // slots first so every expression, binding, and call sees the same result.
        for state in call_inference.iter().flatten() {
            state.finish_projections(&mut table);
        }

        // Keep useful numeric defaults even if some other part of the body hit a work limit.
        // Already solved types stay intact; unresolved general types and conflicts stay unknown.
        for facts in exprs.iter_mut() {
            facts.ty = table.finalize(&facts.ty);
        }
        for ty in binding_tys.iter_mut() {
            *ty = table.finalize(ty);
        }

        // Calls need only their selected function and finalized generic arguments after inference.
        // Collect those in expression-id order, dropping the richer working state as we go.
        let calls = call_inference
            .into_iter()
            .enumerate()
            .filter_map(|(index, state)| state.map(|state| (ExprId(index), state.finalize(&table))))
            .collect();
        BodyFacts::new(binding_tys, exprs, calls)
    }

    #[cfg(test)]
    pub(crate) fn finalize_expr_ty(&self, expr: ExprId) -> Ty {
        self.table.finalize(&self.exprs[expr].ty)
    }

    #[cfg(test)]
    pub(crate) fn finalize_binding_ty(&self, binding: BindingId) -> Ty {
        self.table.finalize(&self.binding_tys[binding])
    }

    fn set_expr_fact(&mut self, expr: ExprId, ty: Ty) {
        Self::set_fact(&self.table, &mut self.exprs[expr].ty, ty);
    }

    fn refine_expr_fact(&mut self, expr: ExprId, ty: Ty) {
        Self::refine_fact(&self.table, &mut self.exprs[expr].ty, ty);
    }

    /// Link earlier live expectations before retaining a producer's shape for consumers.
    ///
    /// A tuple, array, or reference must retain the slots shared with its children. Even if the
    /// parent already has a type without variables, it needs these links to see later evidence.
    pub(crate) fn set_expr_fact_allowing_weak_slot(&mut self, expr: ExprId, ty: Ty) {
        let previous = &self.exprs[expr].ty;
        if previous.has_var() {
            self.table.unify(previous, &ty);
        }
        Self::set_weak_fact(&self.table, &mut self.exprs[expr].ty, ty);
    }

    fn set_binding_fact(&mut self, binding: BindingId, ty: Ty) {
        Self::set_fact(&self.table, &mut self.binding_tys[binding], ty);
    }

    fn refine_binding_fact(&mut self, binding: BindingId, ty: Ty) {
        Self::refine_fact(&self.table, &mut self.binding_tys[binding], ty);
    }

    /// Equal canonical values can still have different live identities. Keep the established
    /// identity so later constraints continue to reach all of its consumers.
    fn set_fact(table: &InferenceTable, slot: &mut Ty, ty: Ty) {
        if table.canonicalize(slot) != table.canonicalize(&ty) {
            *slot = ty;
        }
    }

    fn refine_fact(table: &InferenceTable, slot: &mut Ty, ty: Ty) {
        let ty = InferenceTable::merge_ty_evidence(slot, &ty);
        Self::set_fact(table, slot, ty);
    }

    /// Prefer a type carrying live slots when the stored type has none. Even if both resolve to
    /// the same value now, only the slots will carry later constraints or conflicts.
    fn set_weak_fact(table: &InferenceTable, slot: &mut Ty, ty: Ty) {
        if !slot.has_var() && ty.has_var() {
            *slot = ty;
        } else {
            Self::refine_fact(table, slot, ty);
        }
    }

    /// Give one closure a stable identity plus live slots for every parameter and its return.
    ///
    /// An expected `Fn(User) -> Name` bound and evidence from the closure body must constrain the
    /// same slots. They are allocated when the closure signature is prepared and then carried inside
    /// the closure's semantic type while its body and callable obligations exchange evidence.
    pub(crate) fn set_expr_closure_ty(
        &mut self,
        body_ref: BodyRef,
        expr: ExprId,
        param_count: usize,
    ) {
        let params = (0..param_count)
            .map(|_| self.table.new_type_var())
            .collect();
        let ret = self.table.new_type_var();
        self.set_expr_infer_ty(
            expr,
            Ty::closure(ClosureTyId::new(body_ref, expr), params, ret),
        )
    }

    /// Apply a primitive unary relationship without defaulting a numeric literal prematurely.
    pub(crate) fn set_expr_unary_from_inner(
        &mut self,
        expr: ExprId,
        op: ExprUnaryOp,
        inner: ExprId,
    ) {
        let inner_ty = self.expr_ty(inner);
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
    /// chooses their concrete primitive. An unresolved numeric result shares its operand's slot,
    /// so a later expectation can still constrain the literal.
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

        let lhs_ty = self.expr_ty(lhs);
        let rhs_ty = self.expr_ty(rhs);
        let lhs_resolved = self.table.resolve_root_var(&lhs_ty);
        let rhs_resolved = self.table.resolve_root_var(&rhs_ty);

        let ty = if matches!(op, ExprBinaryOp::Shl | ExprBinaryOp::Shr) {
            // A shift count can have a different integer type, as in `1_u64 << 2_u8`. Only the left
            // operand determines the result, so the operand types must not be unified here.
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
}
