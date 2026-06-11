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
        self.seed_call_return_inference_facts()?;
        self.refresh_inference_dependent_expr_facts();
        self.constrain_expected_types()?;
        self.finalize_facts();
        Ok(())
    }

    /// Seeds inference facts that ordinary `Ty` cannot represent during fixed-point resolution.
    fn seed_call_return_inference_facts(&mut self) -> Result<(), PackageStoreError> {
        for expr_idx in 0..self.pass.body.exprs().len() {
            let expr = ExprId(expr_idx);
            let kind = self.pass.body.expr_unchecked(expr).kind.clone();
            match kind {
                ExprKind::Call { .. } | ExprKind::MethodCall { .. } => {
                    self.seed_call_return_inference_fact(expr)?
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

    fn seed_call_return_inference_fact(&mut self, call: ExprId) -> Result<(), PackageStoreError> {
        if !matches!(self.pass.body.expr_ty_unchecked(call), Ty::Unknown) {
            return Ok(());
        }

        let should_seed_type_var = {
            let calls = self.pass.context().calls();
            let Some(target) = calls.target(call)? else {
                return Ok(());
            };
            calls.signature(&target).can_seed_return_inference()?
        };

        if should_seed_type_var && let Some(inference) = &mut self.pass.inference {
            inference.set_expr_type_var(call);
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
        let param_tys = {
            let calls = self.pass.context().calls();
            let Some(target) = calls.target(call)? else {
                return Ok(());
            };
            calls.signature(&target).param_tys()?
        };
        if param_tys.len() != args.len() {
            return Ok(());
        }

        for (arg, expected_ty) in args.iter().zip(param_tys) {
            self.constrain_expr_with_expected(*arg, &expected_ty);
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
