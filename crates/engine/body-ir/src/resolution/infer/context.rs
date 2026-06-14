use rg_ir_model::items::{GenericParams, TypeRef};
use rg_ir_model::{BindingId, ExprId, ExprWrapperKind};
use rg_ty::{
    Ty,
    inference::{
        ExplicitTypeArgInstantiationBuilder, GenericReturnInstantiationBuilder, InferTy,
        InferenceTable, UnknownTypeInstantiationBuilder,
    },
};

pub(crate) struct BodyInferenceCtx {
    pub(super) table: InferenceTable,
    pub(super) expr_tys: Vec<InferTy>,
    pub(super) binding_tys: Vec<InferTy>,
}

impl BodyInferenceCtx {
    pub(crate) fn new(expr_count: usize, binding_count: usize) -> Self {
        Self {
            table: InferenceTable::new(),
            expr_tys: vec![InferTy::Unknown; expr_count],
            binding_tys: vec![InferTy::Unknown; binding_count],
        }
    }

    pub(crate) fn set_expr_ty(&mut self, expr: ExprId, ty: &Ty) {
        self.expr_tys[expr.0] = InferTy::from_ty(ty);
    }

    pub(crate) fn set_expr_infer_ty(&mut self, expr: ExprId, ty: InferTy) -> bool {
        if self.expr_tys[expr.0] == ty {
            return false;
        }

        self.expr_tys[expr.0] = ty;
        true
    }

    pub(crate) fn expr_ty(&self, expr: ExprId) -> InferTy {
        self.expr_tys[expr.0].clone()
    }

    pub(crate) fn root_resolved_expr_ty(&self, expr: ExprId) -> InferTy {
        self.table.resolve_root_var(&self.expr_tys[expr.0])
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
            self.expr_tys[expr.0] = infer_ty;
        }
        used_vars
    }

    /// Instantiate explicit `_` slots inside a type arg such as `Vec<_>`.
    pub(crate) fn instantiate_explicit_type_arg_ty(
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
            self.expr_tys[expr.0] = infer_ty;
        }
        used_vars
    }

    pub(crate) fn set_expr_integer_var(&mut self, expr: ExprId) {
        self.expr_tys[expr.0] = self.table.new_integer_var();
    }

    pub(crate) fn set_expr_float_var(&mut self, expr: ExprId) {
        self.expr_tys[expr.0] = self.table.new_float_var();
    }

    pub(crate) fn new_type_var(&mut self) -> InferTy {
        self.table.new_type_var()
    }

    pub(crate) fn set_expr_tuple_from_fields(&mut self, expr: ExprId, fields: &[ExprId]) {
        // Tuple expressions carry child slots by value so later expected-type constraints can
        // descend through the tuple and solve literals or variables nested inside each field.
        self.expr_tys[expr.0] = InferTy::Tuple(
            fields
                .iter()
                .map(|field| self.expr_tys[field.0].clone())
                .collect(),
        );
    }

    pub(crate) fn set_expr_array_from_elements(
        &mut self,
        expr: ExprId,
        elements: &[ExprId],
        len: Option<String>,
    ) {
        if elements.is_empty() {
            self.expr_tys[expr.0] = InferTy::Unknown;
            return;
        }

        // Array elements share one element type. Link every element slot through that type so
        // sibling evidence and expected array types can solve literals and generic call results.
        let element_ty = self.table.new_type_var();
        for element in elements {
            let evidence = self.expr_tys[element.0].clone();
            self.table.unify(&element_ty, &evidence);
        }

        self.expr_tys[expr.0] = InferTy::Array {
            inner: Box::new(element_ty),
            len,
        };
    }

    pub(crate) fn set_expr_repeat_array_from_initializer(
        &mut self,
        expr: ExprId,
        initializer: Option<ExprId>,
        len: Option<String>,
    ) {
        let Some(initializer) = initializer else {
            self.expr_tys[expr.0] = InferTy::Unknown;
            return;
        };

        self.expr_tys[expr.0] = InferTy::Array {
            inner: Box::new(self.expr_tys[initializer.0].clone()),
            len,
        };
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
        let inner_ty = self.expr_tys[inner.0].clone();

        self.expr_tys[expr.0] = match kind {
            ExprWrapperKind::Paren | ExprWrapperKind::Await => inner_ty,
            ExprWrapperKind::Ref { mutability } => InferTy::Reference {
                mutability,
                inner: Box::new(inner_ty),
            },
            ExprWrapperKind::Try | ExprWrapperKind::Return => InferTy::from_ty(fallback_ty),
        };
    }

    pub(crate) fn set_expr_block_from_tail(&mut self, expr: ExprId, tail: Option<ExprId>) {
        self.expr_tys[expr.0] = tail
            .map(|tail| self.expr_tys[tail.0].clone())
            .unwrap_or(InferTy::Unit);
    }

    pub(crate) fn set_expr_if_from_branches(
        &mut self,
        expr: ExprId,
        then_branch: Option<ExprId>,
        else_branch: Option<ExprId>,
    ) {
        let Some(else_branch) = else_branch else {
            self.expr_tys[expr.0] = InferTy::Unit;
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
        let result_ty = self.table.new_type_var();
        let mut has_result = false;
        let mut has_value_result = false;
        for result_expr in result_exprs {
            has_result = true;
            let branch_ty = self.table.resolve_root_var(&self.expr_tys[result_expr.0]);
            if matches!(branch_ty, InferTy::Never) {
                continue;
            }

            has_value_result = true;
            self.table
                .unify(&result_ty, &self.expr_tys[result_expr.0].clone());
        }

        self.expr_tys[expr.0] = if has_value_result {
            result_ty
        } else if has_result {
            InferTy::Never
        } else {
            // Note that we don't handle "empty blocks" but "lack of blocks" here,
            // "empty blocks" are handled separately -- these are real exprs that resolve to unit,
            // while here we are dealing with incomplete code like `match` with no arms.
            InferTy::Unknown
        };
    }

    pub(crate) fn set_binding_ty(&mut self, binding: BindingId, ty: &Ty) {
        self.binding_tys[binding.0] = InferTy::from_ty(ty);
    }

    /// Set a binding to an inference-aware type, preserving any previous evidence.
    pub(crate) fn set_binding_infer_ty(&mut self, binding: BindingId, ty: InferTy) -> bool {
        let previous_ty = self.binding_tys[binding.0].clone();
        let changed = self.table.unify(&previous_ty, &ty);
        if previous_ty == ty {
            return changed;
        }

        self.binding_tys[binding.0] = ty;
        true
    }

    /// Copy a binding slot into a path expression that reads it.
    pub(crate) fn set_expr_from_binding(&mut self, expr: ExprId, binding: BindingId) -> bool {
        let ty = self.binding_tys[binding.0].clone();
        if self.expr_tys[expr.0] == ty {
            return false;
        }

        self.expr_tys[expr.0] = ty;
        true
    }

    pub(crate) fn constrain_expr_ty(&mut self, expr: ExprId, expected_ty: &Ty) -> bool {
        self.table.unify(
            &self.expr_tys[expr.0].clone(),
            &InferTy::from_ty(expected_ty),
        )
    }

    pub(crate) fn constrain_expr_infer_ty(&mut self, expr: ExprId, expected_ty: &InferTy) -> bool {
        self.table
            .unify(&self.expr_tys[expr.0].clone(), expected_ty)
    }

    pub(crate) fn constrain_infer_tys(&mut self, lhs: &InferTy, rhs: &InferTy) -> bool {
        self.table.unify(lhs, rhs)
    }

    pub(crate) fn finalize_expr_ty(&self, expr: ExprId) -> Ty {
        self.table.finalize(&self.expr_tys[expr.0])
    }

    pub(crate) fn finalize_binding_ty(&self, binding: BindingId) -> Ty {
        self.table.finalize(&self.binding_tys[binding.0])
    }
}
