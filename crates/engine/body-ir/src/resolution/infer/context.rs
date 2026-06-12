use rg_ir_model::items::{GenericParams, TypeRef};
use rg_ir_model::{BindingId, ExprId, ExprWrapperKind};
use rg_ty::Ty;

use super::{
    instantiate::GenericReturnInstantiationBuilder, model::InferTy, table::InferenceTable,
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

    pub(crate) fn set_expr_integer_var(&mut self, expr: ExprId) {
        self.expr_tys[expr.0] = self.table.new_integer_var();
    }

    pub(crate) fn set_expr_float_var(&mut self, expr: ExprId) {
        self.expr_tys[expr.0] = self.table.new_float_var();
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

    pub(crate) fn set_binding_ty(&mut self, binding: BindingId, ty: &Ty) {
        self.binding_tys[binding.0] = InferTy::from_ty(ty);
    }

    pub(crate) fn constrain_expr_ty(&mut self, expr: ExprId, expected_ty: &Ty) -> bool {
        self.table.unify(
            &self.expr_tys[expr.0].clone(),
            &InferTy::from_ty(expected_ty),
        )
    }

    pub(crate) fn finalize_expr_ty(&self, expr: ExprId) -> Ty {
        self.table.finalize(&self.expr_tys[expr.0])
    }

    pub(crate) fn finalize_binding_ty(&self, binding: BindingId) -> Ty {
        self.table.finalize(&self.binding_tys[binding.0])
    }
}
