//! Expression typing rules over the live body inference slots.
//!
//! Each rule links an expression with its operands or branch results. Repeated pass steps refine
//! those same slots, so evidence from an expected type can flow back through the expression.

use rg_ir_model::{BodyRef, ExprBinaryOp, ExprId, ExprUnaryOp};
use rg_ty::{ClosureTyId, PrimitiveTy, Ty, inference::InferVarKind, ty_for_binary, ty_for_unary};

use crate::body::ExprWrapperKind;

use super::BodyInferenceCtx;

impl BodyInferenceCtx {
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

    pub(crate) fn set_expr_integer_var(&mut self, expr: ExprId) {
        if !matches!(self.expr_ty_ref(expr), Ty::Unknown) {
            return;
        }
        let ty = self.table.new_integer_var();
        self.set_expr_infer_ty(expr, ty);
    }

    pub(crate) fn set_expr_float_var(&mut self, expr: ExprId) {
        if !matches!(self.expr_ty_ref(expr), Ty::Unknown) {
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
        let previous_ty = self.expr_ty(expr);
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
        let element_ty = match self.expr_ty_ref(expr) {
            Ty::Array {
                inner,
                len: existing_len,
            } if existing_len == &len && Self::is_inference_owned_slot(inner) => {
                inner.as_ref().clone()
            }
            _ => self.table.new_type_var(),
        };
        for element in elements {
            let evidence = self.expr_ty(*element);
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
                inner: Box::new(self.expr_ty(initializer)),
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
        let inner_ty = self.expr_ty(inner);

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

        let lhs_ty = self.expr_ty(lhs);
        let rhs_ty = self.expr_ty(rhs);
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
        let ty = tail.map(|tail| self.expr_ty(tail)).unwrap_or(Ty::Unit);
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
        let result_ty = match self.expr_ty_ref(expr) {
            Ty::Unknown | Ty::Never => self.table.new_type_var(),
            ty if Self::is_inference_owned_slot(ty) => ty.clone(),
            _ => self.table.new_type_var(),
        };
        let mut has_result = false;
        let mut has_value_result = false;
        for result_expr in result_exprs {
            has_result = true;
            let branch_ty = self.root_resolved_expr_ty(result_expr);
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

    /// Return whether a fact still points into the inference table.
    fn is_inference_owned_slot(ty: &Ty) -> bool {
        ty.has_var()
    }
}
