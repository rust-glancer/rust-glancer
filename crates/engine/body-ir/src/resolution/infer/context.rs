use rg_ir_model::items::{GenericParams, TypeRef};
use rg_ir_model::{BindingId, ExprId, ExprWrapperKind};

use rg_ty::{
    ClosureTyId, Ty,
    inference::{
        ExplicitTypeArgInstantiationBuilder, GenericReturnInstantiationBuilder, InferTy,
        InferTypeSubst, InferenceTable, UnknownTypeInstantiationBuilder,
    },
};

use super::facts::InferenceFacts;

pub(crate) struct BodyInferenceCtx {
    pub(super) table: InferenceTable,
    expr_tys: InferenceFacts<ExprId>,
    binding_tys: InferenceFacts<BindingId>,
}

impl BodyInferenceCtx {
    pub(crate) fn new(expr_count: usize, binding_count: usize) -> Self {
        Self {
            table: InferenceTable::new(),
            expr_tys: InferenceFacts::new(expr_count),
            binding_tys: InferenceFacts::new(binding_count),
        }
    }

    pub(crate) fn set_expr_ty(&mut self, expr: ExprId, ty: &Ty) {
        self.set_expr_fact(expr, InferTy::from_ty(ty));
    }

    pub(crate) fn set_expr_infer_ty(&mut self, expr: ExprId, ty: InferTy) -> bool {
        self.set_expr_fact(expr, ty)
    }

    pub(crate) fn set_expr_closure_ty(&mut self, expr: ExprId) -> bool {
        // The id intentionally points back to the body expression index. Later callable-trait
        // phases can use that link to recover the closure params/body from the same body arena.
        self.set_expr_infer_ty(expr, InferTy::Closure(ClosureTyId::new(expr)))
    }

    pub(crate) fn expr_ty(&self, expr: ExprId) -> InferTy {
        self.expr_tys.get(expr)
    }

    #[cfg(test)]
    pub(crate) fn binding_ty(&self, binding: BindingId) -> InferTy {
        self.binding_tys.get(binding)
    }

    pub(crate) fn root_resolved_expr_ty(&self, expr: ExprId) -> InferTy {
        self.expr_tys.root_resolved(&self.table, expr)
    }

    pub(crate) fn root_resolved_ty(&self, ty: &InferTy) -> InferTy {
        self.table.resolve_root_var(ty)
    }

    /// Instantiate function type params inside a projected call return.
    pub(crate) fn instantiate_expr_generic_return_ty(
        &mut self,
        expr: ExprId,
        ret_ty: &TypeRef,
        resolved_ty: &Ty,
        generics: &GenericParams,
    ) -> bool {
        let (infer_ty, used_vars) = {
            let mut builder = GenericReturnInstantiationBuilder::new(&mut self.table, generics);
            let infer_ty = builder.ty_from_return(ret_ty, resolved_ty);
            (infer_ty, builder.used_type_vars())
        };

        if used_vars {
            self.set_expr_infer_ty(expr, infer_ty);
        }
        used_vars
    }

    /// Instantiate written `_` slots inside a type ref such as `Vec<_>`.
    pub(crate) fn instantiate_written_infer_ty(
        &mut self,
        arg_ty: &TypeRef,
        resolved_ty: &Ty,
    ) -> (InferTy, bool) {
        let mut builder = ExplicitTypeArgInstantiationBuilder::new(&mut self.table);
        let infer_ty = builder.ty_from_arg(arg_ty, resolved_ty);
        let used_vars = builder.used_type_vars();
        (infer_ty, used_vars)
    }

    /// Instantiate unknowns nested inside a selected call return shape.
    pub(crate) fn instantiate_expr_nested_unknown_ty(&mut self, expr: ExprId, ty: &Ty) -> bool {
        if matches!(ty, Ty::Unknown) {
            return false;
        }

        let (infer_ty, used_vars) = {
            let mut builder = UnknownTypeInstantiationBuilder::new(&mut self.table);
            let infer_ty = builder.ty_from_ty(ty);
            (infer_ty, builder.used_type_vars())
        };

        if used_vars {
            self.set_expr_infer_ty(expr, infer_ty);
        }
        used_vars
    }

    pub(crate) fn set_expr_integer_var(&mut self, expr: ExprId) {
        let ty = self.table.new_integer_var();
        self.set_expr_infer_ty(expr, ty);
    }

    pub(crate) fn set_expr_float_var(&mut self, expr: ExprId) {
        let ty = self.table.new_float_var();
        self.set_expr_infer_ty(expr, ty);
    }

    pub(crate) fn set_expr_tuple_from_fields(&mut self, expr: ExprId, fields: &[ExprId]) {
        // Tuple expressions carry child slots by value so later expected-type constraints can
        // descend through the tuple and solve literals or variables nested inside each field.
        self.set_expr_infer_ty(
            expr,
            InferTy::Tuple(fields.iter().map(|field| self.expr_ty(*field)).collect()),
        );
    }

    pub(crate) fn set_expr_array_from_elements(
        &mut self,
        expr: ExprId,
        elements: &[ExprId],
        len: Option<String>,
    ) {
        if elements.is_empty() {
            self.set_expr_infer_ty(expr, InferTy::Unknown);
            return;
        }

        // Array elements share one element type. Link every element slot through that type so
        // sibling evidence and expected array types can solve literals and generic call results.
        // Refresh may visit the same array many times, so keep the old inference slot when the
        // shape matches.
        let element_ty = match self.expr_tys.get_ref(expr) {
            InferTy::Array {
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
            InferTy::Array {
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
            self.set_expr_infer_ty(expr, InferTy::Unknown);
            return;
        };

        self.set_expr_infer_ty(
            expr,
            InferTy::Array {
                inner: Box::new(self.expr_tys.get(initializer)),
                len,
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
            ExprWrapperKind::Ref { mutability } => InferTy::Reference {
                mutability,
                inner: Box::new(inner_ty),
            },
            ExprWrapperKind::Try | ExprWrapperKind::Return => InferTy::from_ty(fallback_ty),
        };
        self.set_expr_infer_ty(expr, ty);
    }

    pub(crate) fn set_expr_block_from_tail(&mut self, expr: ExprId, tail: Option<ExprId>) {
        let ty = tail
            .map(|tail| self.expr_tys.get(tail))
            .unwrap_or(InferTy::Unit);
        self.set_expr_infer_ty(expr, ty);
    }

    pub(crate) fn set_expr_if_from_branches(
        &mut self,
        expr: ExprId,
        then_branch: Option<ExprId>,
        else_branch: Option<ExprId>,
    ) {
        let Some(else_branch) = else_branch else {
            self.set_expr_infer_ty(expr, InferTy::Unit);
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
        // Refresh may revisit the same branch expression, so reuse only an existing inference slot.
        let result_ty = match self.expr_tys.get_ref(expr) {
            InferTy::Unknown | InferTy::Never => self.table.new_type_var(),
            ty if Self::is_inference_owned_slot(ty) => ty.clone(),
            _ => self.table.new_type_var(),
        };
        let mut has_result = false;
        let mut has_value_result = false;
        for result_expr in result_exprs {
            has_result = true;
            let branch_ty = self.expr_tys.root_resolved(&self.table, result_expr);
            if matches!(branch_ty, InferTy::Never) {
                continue;
            }

            has_value_result = true;
            if matches!(branch_ty, InferTy::Unknown) {
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
            InferTy::Never
        } else {
            // Note that we don't handle "empty blocks" but "lack of blocks" here,
            // "empty blocks" are handled separately -- these are real exprs that resolve to unit,
            // while here we are dealing with incomplete code like `match` with no arms.
            InferTy::Unknown
        };
        self.set_expr_infer_ty(expr, ty);
    }

    pub(crate) fn set_binding_ty(&mut self, binding: BindingId, ty: &Ty) {
        self.set_binding_fact(binding, InferTy::from_ty(ty));
    }

    /// Set a binding to an inference-aware type, preserving any previous evidence.
    pub(crate) fn set_binding_infer_ty(&mut self, binding: BindingId, ty: InferTy) -> bool {
        let previous_ty = self.binding_tys.get(binding);
        let changed = self.table.unify(&previous_ty, &ty);
        self.set_binding_fact(binding, ty) || changed
    }

    /// Copy a binding slot into a path expression that reads it.
    pub(crate) fn set_expr_from_binding(&mut self, expr: ExprId, binding: BindingId) -> bool {
        self.set_expr_fact(expr, self.binding_tys.get(binding))
    }

    pub(crate) fn constrain_expr_ty(&mut self, expr: ExprId, expected_ty: &Ty) -> bool {
        let current_ty = self.expr_ty(expr);
        let changed = self
            .table
            .unify(&current_ty, &InferTy::from_ty(expected_ty));
        self.set_expr_fact(expr, current_ty) || changed
    }

    pub(crate) fn constrain_expr_infer_ty(&mut self, expr: ExprId, expected_ty: &InferTy) -> bool {
        let current_ty = self.expr_ty(expr);
        let changed = self.table.unify(&current_ty, expected_ty);
        self.set_expr_fact(expr, current_ty) || changed
    }

    pub(crate) fn constrain_infer_tys(&mut self, lhs: &InferTy, rhs: &InferTy) -> bool {
        self.table.unify(lhs, rhs)
    }

    pub(crate) fn bind_type_params_from_infer_args(
        &mut self,
        subst: &mut InferTypeSubst,
        generics: &GenericParams,
        args: &[rg_ty::inference::InferGenericArg],
    ) {
        subst.bind_type_params_from_infer_args(&mut self.table, generics, args);
    }

    pub(crate) fn finalize_expr_ty(&self, expr: ExprId) -> Ty {
        self.expr_tys.finalize(&self.table, expr)
    }

    pub(crate) fn finalize_binding_ty(&self, binding: BindingId) -> Ty {
        self.binding_tys.finalize(&self.table, binding)
    }

    /// Return whether a fact still points into the inference table.
    fn is_inference_owned_slot(ty: &InferTy) -> bool {
        ty.has_var()
    }

    /// Compare body-side facts canonically while preserving live inference slots.
    fn set_expr_fact(&mut self, expr: ExprId, ty: InferTy) -> bool {
        self.expr_tys.set(&self.table, expr, ty)
    }

    /// Store a new slot even if its current weak evidence still canonicalizes to the old shape.
    fn set_expr_fact_allowing_weak_slot(&mut self, expr: ExprId, ty: InferTy) -> bool {
        self.expr_tys.set_allowing_weak_slot(&self.table, expr, ty)
    }

    /// Compare binding-side facts canonically while preserving live inference slots.
    fn set_binding_fact(&mut self, binding: BindingId, ty: InferTy) -> bool {
        self.binding_tys.set(&self.table, binding, ty)
    }
}
