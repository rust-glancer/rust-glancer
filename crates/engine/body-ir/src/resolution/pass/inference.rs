//! Inference-facing helpers for the body-resolution pass.
//!
//! Body resolution still publishes ordinary `Ty` facts while it runs. This module collects direct
//! constraints over the parallel inference view and writes the finalized inference facts back into
//! Body IR.

use rg_ir_model::{
    BindingId, ExprId, ScopeId, StmtId,
    identity::DeclarationRef,
    items::{FieldKey, GenericArg as ItemGenericArg, TypeRef},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_ty::Ty;

use crate::{
    ir::{ExprKind, ExprWrapperKind, RecordExprField, StmtKind, resolved::BodyResolution},
    resolution::{TypeRefUseSite, query::SelectedCallable},
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
        self.seed_callable_return_inference_facts()?;
        self.refresh_inference_dependent_expr_facts();
        self.constrain_expected_types()?;
        self.finalize_facts();
        Ok(())
    }

    /// Seeds inference facts that ordinary `Ty` cannot represent during fixed-point resolution.
    fn seed_callable_return_inference_facts(&mut self) -> Result<(), PackageStoreError> {
        for expr_idx in 0..self.pass.body.exprs().len() {
            let expr = ExprId(expr_idx);
            let kind = self.pass.body.expr_unchecked(expr).kind.clone();
            match kind {
                ExprKind::Call { callee, .. } => {
                    self.seed_call_return_inference_fact(expr, callee)?
                }
                ExprKind::MethodCall { generic_args, .. } => {
                    self.seed_method_call_return_inference_fact(expr, &generic_args)?
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Rebuilds expression inference facts that copied child facts before late seeds existed.
    fn refresh_inference_dependent_expr_facts(&mut self) {
        let Some(inference) = &mut self.pass.inference else {
            return;
        };

        for expr_idx in 0..self.pass.body.exprs().len() {
            let expr = ExprId(expr_idx);
            let ExprKind::Tuple { fields } = self.pass.body.expr_unchecked(expr).kind.clone()
            else {
                continue;
            };

            // Tuple facts embed field facts by value. Once generic call results have been seeded
            // as variables, rebuild the tuple so outer tuple constraints can solve those fields.
            inference.set_expr_tuple_from_fields(expr, &fields);
        }
    }

    fn seed_call_return_inference_fact(
        &mut self,
        call: ExprId,
        callee: Option<ExprId>,
    ) -> Result<(), PackageStoreError> {
        if !matches!(self.pass.body.expr_ty_unchecked(call), Ty::Unknown) {
            return Ok(());
        }

        let Some(selected_callable) = self
            .pass
            .context()
            .callable_returns()
            .selected_callable_for_call(callee)?
        else {
            return Ok(());
        };

        self.seed_selected_callable_return_inference_fact(call, &selected_callable)
    }

    fn seed_method_call_return_inference_fact(
        &mut self,
        method_call: ExprId,
        explicit_args: &[ItemGenericArg],
    ) -> Result<(), PackageStoreError> {
        if !matches!(self.pass.body.expr_ty_unchecked(method_call), Ty::Unknown) {
            return Ok(());
        }

        let Some(selected_callable) = self
            .pass
            .context()
            .callable_returns()
            .selected_callable_for_method_call(method_call, explicit_args)?
        else {
            return Ok(());
        };

        self.seed_selected_callable_return_inference_fact(method_call, &selected_callable)
    }

    fn seed_selected_callable_return_inference_fact(
        &mut self,
        expr: ExprId,
        selected_callable: &SelectedCallable,
    ) -> Result<(), PackageStoreError> {
        if !selected_callable.explicit_args().is_empty() {
            return Ok(());
        }

        let should_seed_type_var = if let Some(function_data) = self
            .pass
            .context()
            .item_query()
            .function_data(selected_callable.function_ref())?
            && let Some(generics) = function_data.signature.generics()
            && let Some(ret_ty) = function_data.signature.ret_ty()
            && let Some(ret_name) = ret_ty.type_param_name()
        {
            // `fn make<T>() -> T` has no concrete `Ty` before expected-type constraints run, but
            // inference can preserve the return as `?T` and let the outer expression solve it.
            generics
                .types
                .iter()
                .any(|param| param.name.as_str() == ret_name.as_str())
        } else {
            false
        };

        if should_seed_type_var && let Some(inference) = &mut self.pass.inference {
            inference.set_expr_type_var(expr);
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
            .type_path_query()
            .type_ref(TypeRefUseSite::Scope(scope))
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
            ExprKind::MethodCall {
                receiver: Some(receiver),
                method_name,
                generic_args,
                args,
                ..
            } => self.constrain_method_call_argument_expected_types(
                expr,
                receiver,
                &method_name,
                &generic_args,
                args,
            ),
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
        self.constrain_function_call_argument_expected_types(callee, &args)?;
        self.constrain_enum_variant_payload_expected_types(call, callee, args)
    }

    fn constrain_function_call_argument_expected_types(
        &mut self,
        callee: ExprId,
        args: &[ExprId],
    ) -> Result<(), PackageStoreError> {
        // Only a single resolved function gives us trustworthy parameter evidence. Ambiguous calls
        // keep their already-computed return type but do not push expectations inward.
        let Some(param_tys) = self
            .pass
            .context()
            .callable_returns()
            .function_call_param_tys(callee)?
        else {
            return Ok(());
        };
        if param_tys.len() != args.len() {
            return Ok(());
        }

        for (arg, expected_ty) in args.iter().zip(param_tys) {
            self.constrain_expr_with_expected(*arg, &expected_ty);
        }

        Ok(())
    }

    fn constrain_method_call_argument_expected_types(
        &mut self,
        method_call: ExprId,
        receiver: ExprId,
        method_name: &str,
        explicit_args: &[ItemGenericArg],
        args: Vec<ExprId>,
    ) -> Result<(), PackageStoreError> {
        // Method calls need receiver substitutions before parameter types are useful. The
        // callable query only returns a list when the selected method is unambiguous.
        let Some(param_tys) = self
            .pass
            .context()
            .callable_returns()
            .method_call_param_tys(method_call, receiver, method_name, explicit_args)?
        else {
            return Ok(());
        };
        if param_tys.len() != args.len() {
            return Ok(());
        }

        for (arg, expected_ty) in args.into_iter().zip(param_tys) {
            self.constrain_expr_with_expected(arg, &expected_ty);
        }

        Ok(())
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
        let [DeclarationRef::EnumVariant(variant_ref)] = declarations.as_slice() else {
            return Ok(());
        };
        let variant_ref = *variant_ref;
        let [enum_ty] = self.pass.body.expr_ty_unchecked(call).as_nominals() else {
            return Ok(());
        };
        let enum_ty = enum_ty.clone();

        for (index, arg) in args.into_iter().enumerate() {
            // Enum tuple-variant constructors expose payload fields positionally at the call site.
            // Record variant syntax is a separate expression shape and is intentionally not
            // handled by this hook.
            let field_key = FieldKey::Tuple(index);
            let Some(expected_ty) = self
                .pass
                .context()
                .type_path_query()
                .variant_field_ty_for_enum_variant(&enum_ty, variant_ref, &field_key)?
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
                .type_path_query()
                .field_ty_for_nominal_type(&record_ty, &field.key)?
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

        self.pass
            .context()
            .callable_returns()
            .explicit_declared_return_ty(function)
    }

    fn constrain_root_tail_with_expected(&mut self, expected_ty: &Ty) {
        let ExprKind::Block {
            tail: Some(tail), ..
        } = self
            .pass
            .body
            .expr_unchecked(self.pass.body.root_expr())
            .kind
            .clone()
        else {
            return;
        };

        // `return expr` has type `!`; the wrapped expression is constrained separately below.
        if self.is_explicit_return_expr(tail) {
            return;
        }

        self.constrain_expr_with_expected(tail, expected_ty);
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
            _ => {}
        }
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
