//! Inference transfer rules for the body semantic pass.
//!
//! Syntax-directed expression resolution and these expected-type/obligation rules write into the
//! same `BodyInferenceCtx`. The parent pass owns the only fixed point and asks this module for one
//! transfer step at a time before finalizing persisted facts.

use rg_def_map::DefMapSource;
use rg_ir_model::{
    EnumVariantRef, ExprId, FieldKey, GenericDefRef, PatId, StmtId, identity::DeclarationRef,
};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::{AdtTy, Substitution, Ty};

use crate::{
    ir::{
        ExprAssignOp, ExprKind, ExprWrapperKind, RecordExprField, StmtKind,
        resolved::BodyResolution,
    },
    resolution::infer::{
        BodyCallInference, BodyInferenceSnapshot, BodyMemberInference, BodyPatternInference,
    },
};

use super::body::BodyResolutionPass;

/// Applies one step of inference relationships that complement syntax-directed resolution.
///
/// The parent `BodyResolutionPass` owns convergence. This type only reads one coherent snapshot,
/// writes refinements into the live context, and returns; it never starts a nested fixed point.
pub(super) struct InferenceTransferPass<'pass, 'query, 'body, D, I> {
    pass: &'pass mut BodyResolutionPass<'query, 'body, D, I>,
    snapshot: BodyInferenceSnapshot,
}

impl<'pass, 'query, 'body, D, I> InferenceTransferPass<'pass, 'query, 'body, D, I> {
    pub(super) fn new(pass: &'pass mut BodyResolutionPass<'query, 'body, D, I>) -> Self {
        let snapshot = pass.inference.snapshot();
        Self { pass, snapshot }
    }
}

impl<'pass, 'query, 'body, D, I> InferenceTransferPass<'pass, 'query, 'body, D, I>
where
    for<'source> &'source D: DefMapSource<Error = PackageStoreError>,
    for<'source> &'source I: ItemStoreSource<'source, Error = PackageStoreError>,
{
    /// Allocate identities that must remain stable across every later transfer step.
    ///
    /// Closure types and written `_` positions are body-owned unknowns. Re-lowering either inside
    /// the fixed point would manufacture fresh variables and make unchanged state look new.
    pub(super) fn initialize(mut self) -> Result<(), PackageStoreError> {
        // Give body-owned anonymous types stable identities before calls can use them as generic
        // evidence. Record results also need live slots before annotations reach their fields.
        self.instantiate_closure_type_facts();
        self.instantiate_record_result_facts();

        // Written `_` positions must be lowered once so every later transfer sees the same slot.
        for statement_idx in 0..self.pass.body.statements().len() {
            self.initialize_statement_expected_type(StmtId(statement_idx))?;
        }
        self.constrain_function_return_expected_types()?;
        Ok(())
    }

    /// Apply each inference-only relationship once against the current live facts.
    pub(super) fn apply_once(mut self) -> Result<(), PackageStoreError> {
        self.instantiate_record_result_facts();
        self.project_member_facts()?;
        for expr_idx in 0..self.pass.body.exprs().len() {
            self.constrain_expr_expected_types(ExprId(expr_idx))?;
        }
        for statement_idx in 0..self.pass.body.statements().len() {
            self.constrain_statement_expected_types(StmtId(statement_idx))?;
        }
        self.constrain_function_return_expected_types()?;

        Ok(())
    }

    /// Give every closure expression its own anonymous type and callable inference slots.
    fn instantiate_closure_type_facts(&mut self) {
        for expr_idx in 0..self.pass.body.exprs().len() {
            let expr = ExprId(expr_idx);
            let ExprKind::Closure { params, .. } = &self.pass.body.expr_unchecked(expr).kind else {
                continue;
            };
            self.pass
                .inference
                .set_expr_closure_ty(self.pass.env.body_ref(), expr, params.len());
        }
    }

    /// Turn record literal results such as `Pair<unknown>` into `Pair<?T>`.
    fn instantiate_record_result_facts(&mut self) {
        for expr_idx in 0..self.pass.body.exprs().len() {
            let expr = ExprId(expr_idx);
            if !matches!(
                &self.pass.body.expr_unchecked(expr).kind,
                ExprKind::Record { .. }
            ) {
                continue;
            }

            let ty = self.pass.expr_ty_unchecked(expr).clone();
            if ty.has_unknown() {
                self.pass
                    .inference
                    .instantiate_expr_nested_unknown_ty(expr, &ty);
            }
        }
    }

    /// Turn enum variant constructor results such as `Option<unknown>` into `Option<?T>`.
    fn instantiate_enum_variant_call_result_fact(&mut self, call: ExprId, callee: ExprId) {
        let BodyResolution::Declarations(declarations) = self.pass.expr_resolution(callee) else {
            return;
        };
        let Some(DeclarationRef::EnumVariant(_)) = declarations.as_one() else {
            return;
        };

        let ty = self.pass.expr_ty_unchecked(call).clone();
        self.pass
            .inference
            .instantiate_expr_nested_unknown_ty(call, &ty);
    }

    /// Project field and index expressions from inference-aware bases.
    fn project_member_facts(&mut self) -> Result<(), PackageStoreError> {
        self.pass
            .with_context_and_inference(&self.snapshot, |context, inference| {
                let expr_count = context.body().exprs().len();
                let member_inference = BodyMemberInference::new(context);

                for expr_idx in 0..expr_count {
                    member_inference.project_expr(inference, ExprId(expr_idx))?;
                }

                Ok(())
            })
    }

    /// Transfer one call's return, argument, obligation, and projection evidence as one unit.
    ///
    /// `take_user(value)` makes `value` expect `User`; `id(user)` lets `T` become `User`. The
    /// selected state remains alive while those expectations reach nested argument expressions,
    /// so completion can consume their refined types without selecting the call again.
    fn transfer_call(
        &mut self,
        call: ExprId,
        args: &[ExprId],
        receiver: Option<ExprId>,
    ) -> Result<(), PackageStoreError> {
        let transfer =
            self.pass
                .with_context_and_inference(&self.snapshot, |context, inference| {
                    BodyCallInference::new(context)
                        .prepare_transfer(inference, call, args, receiver)
                })?;
        let Some(transfer) = transfer else {
            return Ok(());
        };

        for (arg, expected_ty) in transfer.argument_expected_tys(args) {
            self.constrain_expr_with_expected(arg, &expected_ty);
        }

        self.pass
            .with_context_and_inference(&self.snapshot, |context, inference| {
                BodyCallInference::new(context).complete_transfer(inference, transfer, args)
            })
    }

    /// Lower each written annotation once, retaining stable variables for `_` positions.
    fn initialize_statement_expected_type(
        &mut self,
        statement: StmtId,
    ) -> Result<(), PackageStoreError> {
        let kind = self.pass.body.statement_unchecked(statement).kind.clone();
        match kind {
            StmtKind::Let {
                scope,
                pat: Some(pat),
                annotation: Some(annotation),
                initializer: Some(initializer),
                ..
            } => {
                let expected_ty = self.pass.with_context_and_inference(
                    &self.snapshot,
                    |context, inference| {
                        context
                            .type_refs(scope)
                            .resolve_with_inference(&annotation, inference.table_mut())
                    },
                )?;
                self.pass
                    .inference
                    .set_statement_expected_ty(statement, expected_ty.clone());
                self.constrain_let_annotation_initializer(pat, initializer, &expected_ty)
            }
            StmtKind::Let { .. }
            | StmtKind::Expr { .. }
            | StmtKind::Item { .. }
            | StmtKind::ItemIgnored => Ok(()),
        }
    }

    /// Reapply the equality between a stable annotation type, its pattern, and initializer.
    fn constrain_statement_expected_types(
        &mut self,
        statement: StmtId,
    ) -> Result<(), PackageStoreError> {
        let StmtKind::Let {
            pat: Some(pat),
            annotation: Some(_),
            initializer: Some(initializer),
            ..
        } = self.pass.body.statement_unchecked(statement).kind.clone()
        else {
            return Ok(());
        };
        let Some(expected_ty) = self.pass.inference.statement_expected_ty(statement) else {
            return Ok(());
        };

        self.constrain_let_annotation_initializer(pat, initializer, &expected_ty)
    }

    /// Constrain an initializer from its explicit statement annotation.
    fn constrain_let_annotation_initializer(
        &mut self,
        pat: PatId,
        initializer: ExprId,
        expected_ty: &Ty,
    ) -> Result<(), PackageStoreError> {
        self.constrain_expr_with_expected(initializer, expected_ty);
        self.pass
            .with_context_and_inference(&self.snapshot, |context, inference| {
                BodyPatternInference::new(context).link_pat(inference, pat, expected_ty)
            })?;

        Ok(())
    }

    /// Route expression-level evidence from calls, method calls, record fields, and assignments.
    fn constrain_expr_expected_types(&mut self, expr: ExprId) -> Result<(), PackageStoreError> {
        let kind = self.pass.body.expr_unchecked(expr).kind.clone();
        match kind {
            ExprKind::Call { callee, args } => {
                self.transfer_call(expr, &args, None)?;
                if let Some(callee) = callee {
                    self.instantiate_enum_variant_call_result_fact(expr, callee);
                    self.constrain_enum_variant_payload_expected_types(expr, callee, args)?;
                }
                Ok(())
            }
            ExprKind::MethodCall { receiver, args, .. } => {
                self.transfer_call(expr, &args, receiver)
            }
            ExprKind::Record { fields, .. } => {
                self.constrain_record_field_initializer_expected_types(expr, fields)
            }
            ExprKind::Assign {
                target: Some(target),
                op: Some(ExprAssignOp::Assign),
                value: Some(value),
            } => {
                self.constrain_simple_assignment(target, value);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Use `target = value` as equality evidence for direct local assignments.
    fn constrain_simple_assignment(&mut self, target: ExprId, value: ExprId) {
        let BodyResolution::Binding(binding) = *self.pass.expr_resolution(target) else {
            return;
        };

        let target_ty = self.pass.inference.root_resolved_expr_ty(target);
        let value_ty = self.pass.inference.root_resolved_expr_ty(value);
        self.pass
            .inference
            .constrain_infer_tys(&target_ty, &value_ty);
        self.pass.inference.set_binding_infer_ty(binding, target_ty);
    }

    /// Use known enum call result to push payload field types into tuple-variant args.
    ///
    /// Example: `Option::Some(value)` with expected `Option<User>` makes `value` expect `User`.
    fn constrain_enum_variant_payload_expected_types(
        &mut self,
        call: ExprId,
        callee: ExprId,
        args: Vec<ExprId>,
    ) -> Result<(), PackageStoreError> {
        let BodyResolution::Declarations(declarations) = self.pass.expr_resolution(callee) else {
            return Ok(());
        };
        let (variant_ref, enum_ty) = if let Some(DeclarationRef::EnumVariant(variant_ref)) =
            declarations.as_one()
            && let [enum_ty] = self.pass.expr_ty_unchecked(call).as_adts()
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
            self.constrain_enum_variant_payload_infer_ty(call, arg, &enum_ty, variant_ref, index)?;
        }

        Ok(())
    }

    /// Use payload args to solve enum generics carried by the constructor result.
    ///
    /// Example: `Option::Some(user)` links variant field `T` to result `Option<?T>`.
    fn constrain_enum_variant_payload_infer_ty(
        &mut self,
        call: ExprId,
        arg: ExprId,
        enum_ty: &AdtTy,
        variant_ref: EnumVariantRef,
        field_index: usize,
    ) -> Result<(), PackageStoreError> {
        let Some(subst) = self.live_nominal_subst(call, enum_ty)? else {
            return Ok(());
        };
        let Some(field_ty) = self
            .pass
            .context()
            .signatures()
            .enum_variant_field_ty(variant_ref, field_index)?
        else {
            return Ok(());
        };
        let expected_ty = subst.apply(&field_ty);
        self.pass.inference.constrain_expr_ty(arg, &expected_ty);
        Ok(())
    }

    /// Bind canonical generic identities from a nominal result such as `Pair<?T>`.
    fn live_nominal_subst(
        &self,
        expr: ExprId,
        nominal_ty: &AdtTy,
    ) -> Result<Option<Substitution>, PackageStoreError> {
        let infer_ty = self.pass.inference.expr_ty(expr);
        let infer_args = match infer_ty {
            Ty::Adt(infer_nominal_ty) if infer_nominal_ty.def == nominal_ty.def => {
                infer_nominal_ty.args
            }
            _ => return Ok(None),
        };
        let generics = self
            .pass
            .context()
            .item_paths()
            .generics()
            .generics(GenericDefRef::TypeDef(nominal_ty.def))?;
        Ok(Some(Substitution::from_args(&generics, &infer_args)))
    }

    /// Use record type and field key to push declared field types into initializers.
    fn constrain_record_field_initializer_expected_types(
        &mut self,
        record: ExprId,
        fields: Vec<RecordExprField>,
    ) -> Result<(), PackageStoreError> {
        let [record_ty] = self.pass.expr_ty_unchecked(record).as_adts() else {
            return Ok(());
        };
        let record_ty = record_ty.clone();

        for field in fields {
            let Some(value) = field.value else {
                continue;
            };
            // Record field initializers are checked against the declared field type, with generic
            // arguments from the record type applied before the expectation reaches the value.
            let Some(target) = self
                .pass
                .context()
                .fields()
                .declared(&record_ty, &field.key)?
            else {
                continue;
            };
            let Some(expected_ty) = target.ty().cloned() else {
                continue;
            };

            self.constrain_expr_with_expected(value, &expected_ty);
            self.constrain_record_field_initializer_infer_ty(
                record,
                value,
                &record_ty,
                target.field(),
            )?;
        }

        Ok(())
    }

    /// Use field initializers to solve generics carried by the record result.
    ///
    /// Example: `Pair { left: user }` links field type `T` to result `Pair<?T>`.
    fn constrain_record_field_initializer_infer_ty(
        &mut self,
        record: ExprId,
        value: ExprId,
        record_ty: &AdtTy,
        field: rg_ir_model::FieldRef,
    ) -> Result<(), PackageStoreError> {
        let Some(subst) = self.live_nominal_subst(record, record_ty)? else {
            return Ok(());
        };
        let Some(field_ty) = self.pass.context().signatures().field_ty(field)? else {
            return Ok(());
        };
        let expected_ty = subst.apply(&field_ty);
        self.pass.inference.constrain_expr_ty(value, &expected_ty);
        Ok(())
    }

    /// Use the declared function return type for the block tail and explicit returns.
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

    /// Resolve `fn f() -> T` for the body owner, if such annotation exists.
    fn explicit_function_return_ty(&self) -> Result<Option<Ty>, PackageStoreError> {
        let Some(function) = self.pass.body.owner().function() else {
            return Ok(None);
        };

        self.pass.context().functions().declared_return_ty(function)
    }

    /// Constrain the root block tail from the function return annotation.
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

    /// Constrain every `return expr` inner expression from the function return annotation.
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

    /// Return expressions have their own wrapper shape and are constrained separately.
    fn is_explicit_return_expr(&self, expr: ExprId) -> bool {
        matches!(
            self.pass.body.expr_unchecked(expr).kind,
            ExprKind::Wrapper {
                kind: ExprWrapperKind::Return,
                ..
            }
        )
    }

    /// Apply an expected type and recurse through transparent shapes like tuples and refs.
    fn constrain_expr_with_expected(&mut self, expr: ExprId, expected_ty: &Ty) {
        if matches!(expected_ty, Ty::Unknown) {
            return;
        }

        self.pass.inference.constrain_expr_ty(expr, expected_ty);

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
                    ..
                },
            ) if mutability == *expected_mutability => {
                self.constrain_expr_with_expected(inner, expected_inner);
            }
            (ExprKind::Block { tail, .. }, _) => {
                self.constrain_optional_result_expr_with_expected(tail, expected_ty);
            }
            (
                ExprKind::If {
                    then_branch,
                    else_branch: Some(else_branch),
                    ..
                },
                _,
            ) => {
                self.constrain_optional_result_expr_with_expected(then_branch, expected_ty);
                self.constrain_result_expr_with_expected(else_branch, expected_ty);
            }
            (ExprKind::Match { arms, .. }, _) => {
                for arm in arms {
                    self.constrain_optional_result_expr_with_expected(arm.expr, expected_ty);
                }
            }
            _ => {}
        }
    }

    /// Constrain an optional expression that contributes to its parent result.
    fn constrain_optional_result_expr_with_expected(
        &mut self,
        expr: Option<ExprId>,
        expected_ty: &Ty,
    ) {
        if let Some(expr) = expr {
            self.constrain_result_expr_with_expected(expr, expected_ty);
        }
    }

    /// Constrain a result expression, skipping explicit `return expr` wrappers.
    fn constrain_result_expr_with_expected(&mut self, expr: ExprId, expected_ty: &Ty) {
        if self.is_explicit_return_expr(expr) {
            return;
        }

        self.constrain_expr_with_expected(expr, expected_ty);
    }

    /// Accept missing array length, otherwise match it against element count.
    fn array_len_matches_count(expected_len: &rg_ty::ConstValue, element_count: usize) -> bool {
        match expected_len {
            rg_ty::ConstValue::Scalar(value) => *value == element_count as u128,
            rg_ty::ConstValue::Param(_) | rg_ty::ConstValue::Unknown => true,
        }
    }

    /// Accept missing array length, otherwise match it against repeat syntax text.
    fn array_len_matches_text(expected_len: &rg_ty::ConstValue, len_text: Option<&str>) -> bool {
        len_text.is_none_or(|actual| match expected_len {
            rg_ty::ConstValue::Scalar(value) => {
                rg_ty::ConstValue::from_syntax(actual) == rg_ty::ConstValue::Scalar(*value)
            }
            rg_ty::ConstValue::Param(_) | rg_ty::ConstValue::Unknown => true,
        })
    }
}
