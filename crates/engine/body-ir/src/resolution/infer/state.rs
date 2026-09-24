//! Live expression facts, binding types, and selected calls for one body.
//!
//! Live facts contain arena handles. An initializer, its binding, and later reads can refer to the
//! same variable; finalization is the only point where those handles become owned semantic types.

use rg_arena::Arena;
use rg_ir_model::{BindingId, BodyRef, ExprBinaryOp, ExprId, ExprUnaryOp};
use rg_ty::{
    ClosureTyId, PrimitiveTy,
    solver::{InferVarKind, InferenceTable, Ty, TyShape},
};

use super::call::CallInferenceState;
use crate::{BodyFacts, ExprFacts, body::facts::BodyResolution};

#[derive(Clone)]
struct LiveExprFacts<'s> {
    resolution: BodyResolution,
    ty: Ty<'s>,
}

/// Facts that can still gain type information while the body is being visited.
///
/// For `let xs = make_vec(); use_vec(xs);`, the call result, `xs`, and its later read can all
/// carry `Vec<?T>`. Learning `?T = u8` through `use_vec` then updates what all of them resolve to.
/// The arenas keep those links alongside declaration resolutions; `finish` writes owned facts.
pub(crate) struct InferenceState<'s> {
    pub(super) table: InferenceTable<'s>,
    call_inference: Vec<Option<CallInferenceState<'s>>>,
    exprs: Arena<ExprId, LiveExprFacts<'s>>,
    binding_tys: Arena<BindingId, Ty<'s>>,
}

impl<'s> InferenceState<'s> {
    pub(crate) fn new(expr_count: usize, binding_count: usize, table: InferenceTable<'s>) -> Self {
        let unknown = table.interner().unknown();
        Self {
            table,
            call_inference: (0..expr_count).map(|_| None).collect(),
            exprs: Arena::from_vec(vec![
                LiveExprFacts {
                    resolution: BodyResolution::Unknown,
                    ty: unknown
                };
                expr_count
            ]),
            binding_tys: Arena::from_vec(vec![unknown; binding_count]),
        }
    }

    pub(crate) fn table(&self) -> &InferenceTable<'s> {
        &self.table
    }

    pub(crate) fn table_mut(&mut self) -> &mut InferenceTable<'s> {
        &mut self.table
    }

    pub(super) fn take_call_inference(&mut self, call: ExprId) -> Option<CallInferenceState<'s>> {
        self.call_inference[call.0].take()
    }

    pub(crate) fn selected_call_function(&self, call: ExprId) -> Option<rg_ir_model::FunctionRef> {
        self.call_inference[call.0]
            .as_ref()
            .map(CallInferenceState::function)
    }

    pub(super) fn set_call_inference(&mut self, call: ExprId, state: CallInferenceState<'s>) {
        self.call_inference[call.0] = Some(state);
    }

    pub(crate) fn call_is_selected(&self, call: ExprId) -> bool {
        self.call_inference[call.0].is_some()
    }

    pub(super) fn call_result_is_pending(&self, call: ExprId, ty: &Ty<'s>) -> bool {
        !self.call_is_selected(call) && self.root_resolved_expr_ty(call) == *ty
    }

    pub(crate) fn call_input(&self, call: ExprId) -> Ty<'s> {
        self.call_inference[call.0]
            .as_ref()
            .map(|s| s.input(self.table.interner()))
            .unwrap_or(self.table.interner().unknown())
    }

    pub(crate) fn expr_ty(&self, expr: ExprId) -> Ty<'s> {
        self.exprs[expr].ty
    }

    pub(crate) fn binding_ty(&self, binding: BindingId) -> Ty<'s> {
        self.binding_tys[binding]
    }

    pub(crate) fn root_resolved_expr_ty(&self, expr: ExprId) -> Ty<'s> {
        self.table.resolve_root_var(self.exprs[expr].ty)
    }

    pub(crate) fn root_resolved_ty(&self, ty: &Ty<'s>) -> Ty<'s> {
        self.table.resolve_root_var(ty)
    }

    pub(crate) fn expr_resolution(&self, expr: ExprId) -> &BodyResolution {
        &self.exprs[expr].resolution
    }

    pub(crate) fn set_expr_resolution(&mut self, expr: ExprId, resolution: BodyResolution) {
        self.exprs[expr].resolution = resolution;
    }

    pub(crate) fn set_expr_facts(&mut self, expr: ExprId, resolution: BodyResolution, ty: Ty<'s>) {
        self.set_expr_ty(expr, ty);
        self.set_expr_resolution(expr, resolution);
    }

    /// Keep the established variable when a later producer supplies its shape. For partially known
    /// producers, give missing children their own slots so useful sibling evidence is retained.
    ///
    /// Replacing a slot containing `?T` with `Vec<u8>` would leave earlier reads of `?T` behind.
    /// Relating them instead lets those reads learn the same answer. A completely unknown slot
    /// has no such identity yet, so its first useful type can be stored directly.
    fn refine(table: &InferenceTable<'s>, slot: &mut Ty<'s>, ty: Ty<'s>) {
        if slot.is_unknown() {
            *slot = ty;
            return;
        }
        if ty.is_unknown() {
            return;
        }
        let existing = if slot.has_unknown() {
            table.instantiate_nested_unknowns(*slot)
        } else {
            *slot
        };
        if table.try_unify(existing, ty).is_ok() {
            if !slot.has_var() {
                *slot = existing;
            }
            if !slot.has_var() && ty.has_var() {
                *slot = ty;
            }
        }
    }

    pub(crate) fn set_expr_ty(&mut self, expr: ExprId, ty: Ty<'s>) {
        Self::refine(&self.table, &mut self.exprs[expr].ty, ty);
    }

    pub(crate) fn set_binding_ty(&mut self, binding: BindingId, ty: Ty<'s>) {
        Self::refine(&self.table, &mut self.binding_tys[binding], ty);
    }

    /// Give an expression a shared destination before its producer has supplied a type.
    /// A plain unknown carries no relationship; a fresh variable lets later evidence flow back.
    pub(crate) fn expr_slot(&mut self, expr: ExprId) -> Ty<'s> {
        if self.exprs[expr].ty.is_unknown() {
            self.exprs[expr].ty = self.table.new_type_var();
        }
        self.exprs[expr].ty
    }

    pub(crate) fn binding_slot(&mut self, binding: BindingId) -> Ty<'s> {
        let current = self.binding_tys[binding];
        if current.is_unknown() {
            self.binding_tys[binding] = self.table.new_type_var();
        } else if current.has_unknown() {
            self.binding_tys[binding] = self.table.instantiate_nested_unknowns(current);
        }
        self.binding_tys[binding]
    }

    pub(crate) fn instantiate_expr_nested_unknown_ty(&mut self, expr: ExprId, ty: &Ty<'s>) {
        if ty.is_unknown() || !ty.has_unknown() {
            return;
        }
        let current = self.root_resolved_expr_ty(expr);
        if !current.is_unknown() && !current.is_var() && !current.has_unknown() {
            self.table.unify(current, ty);
        } else {
            let ty = self.table.instantiate_nested_unknowns(ty);
            self.set_expr_ty(expr, ty);
        }
    }

    pub(crate) fn set_expr_from_binding(&mut self, expr: ExprId, binding: BindingId) {
        let binding_ty = self.binding_slot(binding);
        self.table.unify(self.exprs[expr].ty, binding_ty);
        self.exprs[expr].ty = binding_ty;
    }

    pub(crate) fn constrain_expr_ty(&mut self, expr: ExprId, expected: &Ty<'s>) {
        // `!` coerces to the destination without equating that destination to `!`.
        if self.root_resolved_expr_ty(expr).is_never()
            && !self.table.resolve_root_var(expected).is_never()
        {
            return;
        }
        self.set_expr_ty(expr, *expected);
    }

    /// A block may coerce its diverging tail to a value type. Keep this adjustment on the block;
    /// changing the tail's shared variable would incorrectly make the diverging call return it.
    pub(crate) fn set_coerced_expr_ty(&mut self, expr: ExprId, ty: Ty<'s>) {
        self.exprs[expr].ty = ty;
    }

    pub(crate) fn constrain_infer_tys(&mut self, lhs: &Ty<'s>, rhs: &Ty<'s>) {
        self.table.unify(lhs, rhs);
    }

    /// Resolve every live handle while its table still exists. Only owned types, declaration ids,
    /// and the selected calls' final generic arguments are retained in the published body.
    pub(crate) fn finish(self) -> BodyFacts {
        let Self {
            table,
            call_inference,
            exprs,
            binding_tys,
        } = self;
        let exprs = Arena::from_vec(
            exprs
                .into_vec()
                .into_iter()
                .map(|facts| ExprFacts {
                    resolution: facts.resolution,
                    ty: table.finalize(facts.ty),
                })
                .collect(),
        );
        let binding_tys = Arena::from_vec(
            binding_tys
                .into_vec()
                .into_iter()
                .map(|ty| table.finalize(ty))
                .collect(),
        );
        let calls = call_inference
            .into_iter()
            .enumerate()
            .filter_map(|(index, state)| state.map(|s| (ExprId(index), s.finalize(&table))))
            .collect();
        BodyFacts::new(binding_tys, exprs, calls)
    }

    #[cfg(test)]
    pub(crate) fn finalize_expr_ty(&self, expr: ExprId) -> rg_ty::Ty {
        self.table.finalize(self.exprs[expr].ty)
    }

    #[cfg(test)]
    pub(crate) fn finalize_binding_ty(&self, binding: BindingId) -> rg_ty::Ty {
        self.table.finalize(self.binding_tys[binding])
    }

    pub(crate) fn set_expr_closure_ty(&mut self, body: BodyRef, expr: ExprId, count: usize) {
        let params = (0..count)
            .map(|_| self.table.new_type_var())
            .collect::<Vec<_>>();
        let ret = self.table.new_type_var();
        let ty = self
            .table
            .interner()
            .closure(ClosureTyId::new(body, expr), &params, ret);
        self.set_expr_ty(expr, ty);
    }

    pub(crate) fn set_expr_unary_from_inner(
        &mut self,
        expr: ExprId,
        op: ExprUnaryOp,
        inner: ExprId,
    ) {
        let ty = self.root_resolved_expr_ty(inner);
        let supported = match (ty.shape(), op) {
            (
                TyShape::InferVar {
                    kind: InferVarKind::Integer,
                },
                ExprUnaryOp::Not | ExprUnaryOp::Neg,
            )
            | (
                TyShape::InferVar {
                    kind: InferVarKind::Float,
                },
                ExprUnaryOp::Neg,
            ) => true,
            (TyShape::Primitive(p), _) => !matches!(
                rg_ty::ty_for_unary(op, &rg_ty::Ty::Primitive(p)),
                rg_ty::Ty::Unknown
            ),
            _ => false,
        };
        if supported {
            self.set_expr_ty(expr, self.expr_ty(inner));
        }
    }

    pub(crate) fn set_expr_binary_from_operands(
        &mut self,
        expr: ExprId,
        op: ExprBinaryOp,
        lhs: ExprId,
        rhs: ExprId,
    ) {
        if op.is_logical() || op.is_comparison() {
            self.set_expr_ty(expr, self.table.interner().primitive(PrimitiveTy::Bool));
            return;
        }
        let left = self.root_resolved_expr_ty(lhs);
        let right = self.root_resolved_expr_ty(rhs);
        let integral = |ty: Ty<'s>| {
            matches!(
                ty.shape(),
                TyShape::InferVar {
                    kind: InferVarKind::Integer
                }
            ) || matches!(ty.shape(), TyShape::Primitive(p) if p.is_integral())
        };
        if matches!(op, ExprBinaryOp::Shl | ExprBinaryOp::Shr) {
            if integral(left) && (integral(right) || right.is_unknown()) {
                self.set_expr_ty(expr, self.expr_ty(lhs));
            }
            return;
        }
        let accepts = |ty: Ty<'s>| match ty.shape() {
            TyShape::Primitive(p) => {
                if matches!(
                    op,
                    ExprBinaryOp::BitAnd | ExprBinaryOp::BitOr | ExprBinaryOp::BitXor
                ) {
                    p.is_integral() || p.is_bool()
                } else {
                    p.is_numeric()
                }
            }
            TyShape::InferVar { kind } => {
                kind == InferVarKind::Integer
                    || kind == InferVarKind::Float
                        && !matches!(
                            op,
                            ExprBinaryOp::BitAnd | ExprBinaryOp::BitOr | ExprBinaryOp::BitXor
                        )
            }
            _ => false,
        };
        if accepts(left) && accepts(right) && self.table.try_unify(left, right).is_ok() {
            self.set_expr_ty(expr, self.expr_ty(lhs));
        }
    }
}
