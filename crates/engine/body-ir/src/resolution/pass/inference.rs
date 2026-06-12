//! Inference-facing helpers for the body-resolution pass.
//!
//! Body resolution still publishes ordinary `Ty` facts while it runs. This module collects direct
//! constraints over the parallel inference view and writes the finalized inference facts back into
//! Body IR.

use rg_ir_model::{
    BindingId, ExprId, ScopeId, StmtId,
    identity::DeclarationRef,
    items::{FieldKey, TypeRef},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_ty::Ty;

use crate::{
    ir::{ExprKind, ExprWrapperKind, RecordExprField, StmtKind, resolved::BodyResolution},
    resolution::TypeRefUseSite,
};

use super::body::BodyResolutionPass;

/// Collects body-local inference constraints that need the fixed-point facts to be available.
///
/// The pass runs after normal body resolution has stabilized: it adds direct constraints such as
/// annotated `let` initializers, then collapses inference variables back into ordinary `Ty` facts.
pub(super) struct InferenceResolutionPass<'pass, 'query, 'body, D, I> {
    pass: &'pass mut BodyResolutionPass<'query, 'body, D, I>,
}

impl<'pass, 'query, 'body, D, I> InferenceResolutionPass<'pass, 'query, 'body, D, I> {
    pub(super) fn new(pass: &'pass mut BodyResolutionPass<'query, 'body, D, I>) -> Self {
        Self { pass }
    }
}

impl<'pass, 'query, 'body, D, I> InferenceResolutionPass<'pass, 'query, 'body, D, I>
where
    for<'source> &'source D: DefMapSource<Error = PackageStoreError>,
    for<'source> &'source I: ItemStoreSource<'source, Error = PackageStoreError>,
{
    pub(super) fn run(mut self) -> Result<(), PackageStoreError> {
        // 1. Mark `T` as `?T` in contexts where local evidence may infer it later.
        // Without this step, those positions stay as plain `Ty::Unknown`.
        self.instantiate_inference_facts()?;

        // 2. Propagate `?` markers through expressions that depend on instantiated children.
        self.refresh_inference_dependent_expr_facts();

        // 3. Run inference: observe available evidence and solve `?T` where possible.
        self.constrain_expected_types()?;

        // 4. Write inferred facts back into Body IR as ordinary `Ty` values.
        self.finalize_facts();
        Ok(())
    }

    /// Instantiate all inference facts that ordinary `Ty` cannot represent.
    fn instantiate_inference_facts(&mut self) -> Result<(), PackageStoreError> {
        self.instantiate_generic_call_return_facts()
    }

    /// Instantiate generic call returns such as `Vec<T>` as `Vec<?T>`.
    fn instantiate_generic_call_return_facts(&mut self) -> Result<(), PackageStoreError> {
        for expr_idx in 0..self.pass.body.exprs().len() {
            let expr = ExprId(expr_idx);
            let kind = self.pass.body.expr_unchecked(expr).kind.clone();
            match kind {
                ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => {
                    self.instantiate_generic_call_return_fact(expr, &args)?
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Refresh expression facts that copied child facts before instantiation.
    fn refresh_inference_dependent_expr_facts(&mut self) {
        let Some(inference) = &mut self.pass.inference else {
            return;
        };

        for expr_idx in 0..self.pass.body.exprs().len() {
            let expr = ExprId(expr_idx);
            let kind = self.pass.body.expr_unchecked(expr).kind.clone();
            match kind {
                ExprKind::Tuple { fields } => inference.set_expr_tuple_from_fields(expr, &fields),
                ExprKind::Array { elements } => inference.set_expr_array_from_elements(
                    expr,
                    &elements,
                    Some(elements.len().to_string()),
                ),
                ExprKind::RepeatArray {
                    initializer,
                    len_text,
                    ..
                } => {
                    inference.set_expr_repeat_array_from_initializer(expr, initializer, len_text);
                }
                ExprKind::Wrapper { kind, inner } => {
                    let fallback_ty = self.pass.body.expr_ty_unchecked(expr).clone();
                    inference.set_expr_wrapper_from_inner(expr, kind, inner, &fallback_ty);
                }
                _ => {}
            }
        }
    }

    /// Instantiate one generic call return when it still contains unknown type params.
    fn instantiate_generic_call_return_fact(
        &mut self,
        call: ExprId,
        args: &[ExprId],
    ) -> Result<(), PackageStoreError> {
        if !self.pass.body.expr_ty_unchecked(call).has_unknown() {
            return Ok(());
        }

        let projection = {
            let calls = self.pass.context().calls();
            let Some(target) = calls.target(call)? else {
                return Ok(());
            };
            calls.signature(&target).project(args)?
        };

        if projection.explicit_args().is_empty()
            && let Some(ret_ty) = projection.declared_return_ty()
            && let Some(generics) = projection.function_generics()
        {
            let type_params = generics
                .types
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>();
            if !ret_ty.mentions_type_param(&type_params) {
                return Ok(());
            }

            let Some(inference) = &mut self.pass.inference else {
                return Ok(());
            };
            inference.instantiate_expr_generic_return_ty(
                call,
                ret_ty,
                projection.return_ty(),
                generics,
            );
        }

        Ok(())
    }

    fn constrain_call_target_argument_expected_types(
        &mut self,
        call: ExprId,
        args: &[ExprId],
    ) -> Result<(), PackageStoreError> {
        // Only a single resolved function gives us trustworthy parameter evidence. Ambiguous calls
        // keep their already-computed return type but do not push expectations inward.
        let projection = {
            let calls = self.pass.context().calls();
            let Some(target) = calls.target(call)? else {
                return Ok(());
            };
            calls.signature(&target).project(args)?
        };
        if projection.written_param_tys().len() != args.len() {
            return Ok(());
        }

        for (arg, expected_ty) in args.iter().zip(projection.written_param_tys()) {
            self.constrain_expr_with_expected(*arg, expected_ty);
        }

        Ok(())
    }

    /// Walks direct expected-type sources and lets expression hooks push them inward.
    fn constrain_expected_types(&mut self) -> Result<(), PackageStoreError> {
        for statement_idx in 0..self.pass.body.statements().len() {
            self.constrain_statement_expected_types(StmtId(statement_idx))?;
        }
        for expr_idx in 0..self.pass.body.exprs().len() {
            self.constrain_expr_expected_types(ExprId(expr_idx))?;
        }
        self.constrain_function_return_expected_types()?;

        Ok(())
    }

    fn constrain_statement_expected_types(
        &mut self,
        statement: StmtId,
    ) -> Result<(), PackageStoreError> {
        let kind = self.pass.body.statement_unchecked(statement).kind.clone();
        match kind {
            StmtKind::Let {
                scope,
                annotation: Some(annotation),
                initializer: Some(initializer),
                ..
            } => self.constrain_let_annotation_initializer(scope, annotation, initializer),
            StmtKind::Let { .. }
            | StmtKind::Expr { .. }
            | StmtKind::Item { .. }
            | StmtKind::ItemIgnored => Ok(()),
        }
    }

    /// Constrains an initializer expression from its explicit statement annotation.
    ///
    /// This intentionally only links the annotation to the initializer root expression. Nested
    /// expected-type propagation belongs in expression kind-specific rules.
    fn constrain_let_annotation_initializer(
        &mut self,
        scope: ScopeId,
        annotation: TypeRef,
        initializer: ExprId,
    ) -> Result<(), PackageStoreError> {
        let expected_ty = self
            .pass
            .context()
            .type_refs(TypeRefUseSite::Scope(scope))
            .resolve(&annotation)?;
        self.constrain_expr_with_expected(initializer, &expected_ty);

        Ok(())
    }

    fn constrain_expr_expected_types(&mut self, expr: ExprId) -> Result<(), PackageStoreError> {
        let kind = self.pass.body.expr_unchecked(expr).kind.clone();
        match kind {
            ExprKind::Call {
                callee: Some(callee),
                args,
            } => self.constrain_call_argument_expected_types(expr, callee, args),
            ExprKind::MethodCall { args, .. } => {
                self.constrain_call_target_argument_expected_types(expr, &args)
            }
            ExprKind::Record { fields, .. } => {
                self.constrain_record_field_initializer_expected_types(expr, fields)
            }
            _ => Ok(()),
        }
    }

    fn constrain_call_argument_expected_types(
        &mut self,
        call: ExprId,
        callee: ExprId,
        args: Vec<ExprId>,
    ) -> Result<(), PackageStoreError> {
        self.constrain_call_target_argument_expected_types(call, &args)?;
        self.constrain_enum_variant_payload_expected_types(call, callee, args)
    }

    fn constrain_enum_variant_payload_expected_types(
        &mut self,
        call: ExprId,
        callee: ExprId,
        args: Vec<ExprId>,
    ) -> Result<(), PackageStoreError> {
        let BodyResolution::Declarations(declarations) = self.pass.body.expr_resolution(callee)
        else {
            return Ok(());
        };
        let (variant_ref, enum_ty) = if let [DeclarationRef::EnumVariant(variant_ref)] =
            declarations.as_slice()
            && let [enum_ty] = self.pass.body.expr_ty_unchecked(call).as_nominals()
        {
            (*variant_ref, enum_ty.clone())
        } else {
            return Ok(());
        };

        for (index, arg) in args.into_iter().enumerate() {
            // Enum tuple-variant constructors expose payload fields positionally at the call site.
            // Record variant syntax is a separate expression shape and is intentionally not
            // handled by this hook.
            let field_key = FieldKey::Tuple(index);
            let Some(expected_ty) = self.pass.context().fields().enum_variant_field_ty(
                &enum_ty,
                variant_ref,
                &field_key,
            )?
            else {
                continue;
            };

            self.constrain_expr_with_expected(arg, &expected_ty);
        }

        Ok(())
    }

    fn constrain_record_field_initializer_expected_types(
        &mut self,
        record: ExprId,
        fields: Vec<RecordExprField>,
    ) -> Result<(), PackageStoreError> {
        let [record_ty] = self.pass.body.expr_ty_unchecked(record).as_nominals() else {
            return Ok(());
        };
        let record_ty = record_ty.clone();

        for field in fields {
            let Some(value) = field.value else {
                continue;
            };
            // Record field initializers are checked against the declared field type, with generic
            // arguments from the record type applied before the expectation reaches the value.
            let Some(expected_ty) = self
                .pass
                .context()
                .fields()
                .declared(&record_ty, &field.key)?
                .and_then(|target| target.ty().cloned())
            else {
                continue;
            };

            self.constrain_expr_with_expected(value, &expected_ty);
        }

        Ok(())
    }

    fn constrain_function_return_expected_types(&mut self) -> Result<(), PackageStoreError> {
        let Some(expected_ty) = self.explicit_function_return_ty()? else {
            return Ok(());
        };

        // A function return annotation applies to two syntactic shapes: the root block tail and
        // every explicit `return expr`. Both feed into the same expression-level propagation.
        self.constrain_root_tail_with_expected(&expected_ty);
        self.constrain_explicit_returns_with_expected(&expected_ty);
        Ok(())
    }

    fn explicit_function_return_ty(&self) -> Result<Option<Ty>, PackageStoreError> {
        let Some(function) = self.pass.body.function_owner() else {
            return Ok(None);
        };

        self.pass.context().functions().declared_return_ty(function)
    }

    fn constrain_root_tail_with_expected(&mut self, expected_ty: &Ty) {
        // `return expr` has type `!`; the wrapped expression is constrained separately below.
        if let ExprKind::Block {
            tail: Some(tail), ..
        } = self
            .pass
            .body
            .expr_unchecked(self.pass.body.root_expr())
            .kind
            .clone()
            && !self.is_explicit_return_expr(tail)
        {
            self.constrain_expr_with_expected(tail, expected_ty);
        }
    }

    fn constrain_explicit_returns_with_expected(&mut self, expected_ty: &Ty) {
        for expr_idx in 0..self.pass.body.exprs().len() {
            let expr = ExprId(expr_idx);
            let ExprKind::Wrapper {
                kind: ExprWrapperKind::Return,
                inner: Some(inner),
            } = self.pass.body.expr_unchecked(expr).kind.clone()
            else {
                continue;
            };

            self.constrain_expr_with_expected(inner, expected_ty);
        }
    }

    fn is_explicit_return_expr(&self, expr: ExprId) -> bool {
        matches!(
            self.pass.body.expr_unchecked(expr).kind,
            ExprKind::Wrapper {
                kind: ExprWrapperKind::Return,
                ..
            }
        )
    }

    /// Applies an expected type to one expression and descends through shapes that preserve it.
    fn constrain_expr_with_expected(&mut self, expr: ExprId, expected_ty: &Ty) {
        if matches!(expected_ty, Ty::Unknown) {
            return;
        }

        if let Some(inference) = &mut self.pass.inference {
            inference.constrain_expr_ty(expr, expected_ty);
        }

        let kind = self.pass.body.expr_unchecked(expr).kind.clone();
        match (kind, expected_ty) {
            (ExprKind::Tuple { fields }, Ty::Tuple(expected_fields))
                if fields.len() == expected_fields.len() =>
            {
                for (field, expected_field) in fields.into_iter().zip(expected_fields) {
                    self.constrain_expr_with_expected(field, expected_field);
                }
            }
            (ExprKind::Array { elements }, Ty::Array { inner, len })
                if Self::array_len_matches_count(len, elements.len()) =>
            {
                for element in elements {
                    self.constrain_expr_with_expected(element, inner);
                }
            }
            (
                ExprKind::RepeatArray {
                    initializer: Some(initializer),
                    len_text,
                    ..
                },
                Ty::Array { inner, len },
            ) if Self::array_len_matches_text(len, len_text.as_deref()) => {
                self.constrain_expr_with_expected(initializer, inner);
            }
            (
                ExprKind::Wrapper {
                    kind: ExprWrapperKind::Paren | ExprWrapperKind::Await,
                    inner: Some(inner),
                },
                _,
            ) => {
                self.constrain_expr_with_expected(inner, expected_ty);
            }
            (
                ExprKind::Wrapper {
                    kind: ExprWrapperKind::Ref { mutability },
                    inner: Some(inner),
                },
                Ty::Reference {
                    mutability: expected_mutability,
                    inner: expected_inner,
                },
            ) if mutability == *expected_mutability => {
                self.constrain_expr_with_expected(inner, expected_inner);
            }
            _ => {}
        }
    }

    fn array_len_matches_count(expected_len: &Option<String>, element_count: usize) -> bool {
        expected_len
            .as_deref()
            .is_none_or(|len| len == element_count.to_string())
    }

    fn array_len_matches_text(expected_len: &Option<String>, len_text: Option<&str>) -> bool {
        expected_len
            .as_deref()
            .is_none_or(|expected| len_text.is_none_or(|actual| actual == expected))
    }

    /// Writes finalized inference facts back into the body.
    ///
    /// Downstream queries only read `Ty`, so unresolved variables are defaulted or erased here
    /// before the resolved body leaves this pass.
    fn finalize_facts(&mut self) {
        let Some(inference) = self.pass.inference.take() else {
            return;
        };

        // Persist the inference view back into Body IR so downstream queries see ordinary `Ty`
        // facts. Unsolved numeric variables become defaults; conflicts become `<unknown>`.
        for expr_idx in 0..self.pass.body.exprs().len() {
            let expr = ExprId(expr_idx);
            self.pass
                .body
                .set_expr_ty(expr, inference.finalize_expr_ty(expr));
        }
        for binding_idx in 0..self.pass.body.bindings().len() {
            let binding = BindingId(binding_idx);
            self.pass
                .body
                .set_binding_ty(binding, inference.finalize_binding_ty(binding));
        }
    }
}
