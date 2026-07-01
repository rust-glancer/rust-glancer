//! Inference-facing helpers for the body-resolution pass.
//!
//! Body resolution still publishes ordinary `Ty` facts while it runs. This module collects direct
//! constraints over the parallel inference view and writes the finalized inference facts back into
//! Body IR.

use rg_ir_model::{
    BindingId, EnumVariantRef, ExprId, PatId, ScopeId, StmtId,
    identity::DeclarationRef,
    items::{FieldKey, FieldList, GenericParams, TypeRef},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_ty::{
    NominalTy, Ty,
    inference::{InferTy, InferTypeRefProjector, InferTypeSubst},
};

use crate::{
    ir::{
        ExprAssignOp, ExprKind, ExprWrapperKind, PatKind, RecordExprField, StmtKind,
        resolved::BodyResolution,
    },
    resolution::{
        TypeRefUseSite,
        infer::{BodyCallInference, BodyMemberInference, BodyPatternInference},
    },
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
        self.refresh_inference_dependent_expr_facts()?;

        // 3. Run inference: observe available evidence and solve `?T` where possible.
        self.constrain_expected_types()?;

        // 4. Write inferred facts back into Body IR as ordinary `Ty` values.
        self.finalize_facts();
        Ok(())
    }

    /// Instantiate facts that need the inference pass to add body-local identity or slots.
    fn instantiate_inference_facts(&mut self) -> Result<(), PackageStoreError> {
        // TODO: These could be one body walk, but later instantiation steps can depend on facts
        // from earlier ones. For example, call instantiation may eventually need closure
        // witnesses to exist before processing a call argument. We keep the passes explicit until
        // this code is more mature and there is a clearer reason to optimize the extra scans.
        self.instantiate_closure_type_facts();
        self.instantiate_generic_call_result_facts()?;
        self.instantiate_record_result_facts();
        Ok(())
    }

    /// Give every closure expression its own anonymous body-local type.
    fn instantiate_closure_type_facts(&mut self) {
        for expr_idx in 0..self.pass.body.exprs().len() {
            let expr = ExprId(expr_idx);
            if matches!(
                &self.pass.body.expr_unchecked(expr).kind,
                ExprKind::Closure { .. }
            ) {
                self.pass.inference.set_expr_closure_ty(expr);
            }
        }
    }

    /// Turn generic call results such as `Vec<T>` or `Option<T>` into `Vec<?T>` / `Option<?T>`.
    fn instantiate_generic_call_result_facts(&mut self) -> Result<(), PackageStoreError> {
        for expr_idx in 0..self.pass.body.exprs().len() {
            let expr = ExprId(expr_idx);
            let kind = self.pass.body.expr_unchecked(expr).kind.clone();
            match kind {
                ExprKind::Call { callee, args } => {
                    let context = self.pass.providers.context(self.pass.body);
                    BodyCallInference::new(context).instantiate_return_fact(
                        &mut self.pass.inference,
                        expr,
                        &args,
                        None,
                    )?;
                    if let Some(callee) = callee {
                        self.instantiate_enum_variant_call_result_fact(expr, callee);
                    }
                }
                ExprKind::MethodCall { receiver, args, .. } => {
                    let context = self.pass.providers.context(self.pass.body);
                    BodyCallInference::new(context).instantiate_return_fact(
                        &mut self.pass.inference,
                        expr,
                        &args,
                        receiver,
                    )?;
                }
                _ => {}
            }
        }

        Ok(())
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

            let ty = self.pass.body.expr_ty_unchecked(expr).clone();
            if ty.has_unknown() {
                self.pass
                    .inference
                    .instantiate_expr_nested_unknown_ty(expr, &ty);
            }
        }
    }

    /// Turn enum variant constructor results such as `Option<unknown>` into `Option<?T>`.
    fn instantiate_enum_variant_call_result_fact(&mut self, call: ExprId, callee: ExprId) {
        let BodyResolution::Declarations(declarations) = self.pass.body.expr_resolution(callee)
        else {
            return;
        };
        let Some(DeclarationRef::EnumVariant(_)) = declarations.as_one() else {
            return;
        };

        let ty = self.pass.body.expr_ty_unchecked(call).clone();
        self.pass
            .inference
            .instantiate_expr_nested_unknown_ty(call, &ty);
    }

    /// Rebuild copied expression facts after child slots may have gained `?T`.
    fn refresh_inference_dependent_expr_facts(&mut self) -> Result<(), PackageStoreError> {
        let max_passes = self.pass.body.bindings().len() + self.pass.body.exprs().len() + 1;
        for _ in 0..max_passes {
            self.refresh_shape_expr_facts();

            let mut changed = false;
            changed |= self.refresh_member_projection_facts()?;
            changed |= self.refresh_binding_flow_facts();
            if !changed {
                break;
            }
        }

        Ok(())
    }

    /// Make `let second = first;` chains share one inference slot graph.
    fn refresh_binding_flow_facts(&mut self) -> bool {
        // Binding reads and binding initializers can form short chains such as
        // `let second = first;`. Iterate over this narrow graph so every slot shares the same
        // inference vars before expected-type constraints run.
        let mut any_changed = false;
        let max_passes = self.pass.body.bindings().len() + self.pass.body.exprs().len() + 1;
        for _ in 0..max_passes {
            let mut changed = false;
            changed |= self.link_let_binding_initializers();
            changed |= self.refresh_binding_path_expr_facts();
            any_changed |= changed;
            if !changed {
                break;
            }
        }

        any_changed
    }

    /// Visit every unannotated `let pat = expr` that can carry initializer evidence.
    fn link_let_binding_initializers(&mut self) -> bool {
        let context = self.pass.providers.context(self.pass.body);
        let pattern_inference = BodyPatternInference::new(context);
        let mut changed = false;

        for statement_idx in 0..self.pass.body.statements().len() {
            let StmtKind::Let {
                pat: Some(pat),
                annotation: None,
                initializer: Some(initializer),
                ..
            } = self
                .pass
                .body
                .statement_unchecked(StmtId(statement_idx))
                .kind
                .clone()
            else {
                continue;
            };
            changed |= pattern_inference.link_initializer_pattern(
                &mut self.pass.inference,
                pat,
                initializer,
            );
        }

        for expr_idx in 0..self.pass.body.exprs().len() {
            let expr = ExprId(expr_idx);
            let ExprKind::Let {
                pat: Some(pat),
                initializer: Some(initializer),
                ..
            } = self.pass.body.expr_unchecked(expr).kind.clone()
            else {
                continue;
            };
            changed |= pattern_inference.link_initializer_pattern(
                &mut self.pass.inference,
                pat,
                initializer,
            );
        }

        changed
    }

    /// Copy binding slots back into local reads such as `values` or `alias`.
    fn refresh_binding_path_expr_facts(&mut self) -> bool {
        let mut changed = false;

        for expr_idx in 0..self.pass.body.exprs().len() {
            let expr = ExprId(expr_idx);
            if let BodyResolution::Binding(binding) = self.pass.body.expr_resolution(expr) {
                changed |= self.pass.inference.set_expr_from_binding(expr, *binding);
            }
        }

        changed
    }

    /// Rebuild field and index expressions that project out of inference-aware bases.
    fn refresh_member_projection_facts(&mut self) -> Result<bool, PackageStoreError> {
        let context = self.pass.providers.context(self.pass.body);
        let member_inference = BodyMemberInference::new(context);
        let mut changed = false;

        for expr_idx in 0..self.pass.body.exprs().len() {
            changed |= member_inference
                .refresh_projection_fact(&mut self.pass.inference, ExprId(expr_idx))?;
        }

        Ok(changed)
    }

    /// Rebuild shapes such as `(?T,)`, `[?T; N]`, `&?T`, or branch results from child slots.
    fn refresh_shape_expr_facts(&mut self) {
        for expr_idx in 0..self.pass.body.exprs().len() {
            let expr = ExprId(expr_idx);
            let kind = self.pass.body.expr_unchecked(expr).kind.clone();
            match kind {
                ExprKind::Tuple { fields } => self
                    .pass
                    .inference
                    .set_expr_tuple_from_fields(expr, &fields),
                ExprKind::Array { elements } => self.pass.inference.set_expr_array_from_elements(
                    expr,
                    &elements,
                    Some(elements.len().to_string()),
                ),
                ExprKind::RepeatArray {
                    initializer,
                    len_text,
                    ..
                } => {
                    self.pass.inference.set_expr_repeat_array_from_initializer(
                        expr,
                        initializer,
                        len_text,
                    );
                }
                ExprKind::Wrapper { kind, inner } => {
                    let fallback_ty = self.pass.body.expr_ty_unchecked(expr).clone();
                    self.pass.inference.set_expr_wrapper_from_inner(
                        expr,
                        kind,
                        inner,
                        &fallback_ty,
                    );
                }
                ExprKind::Block {
                    statements, tail, ..
                } => {
                    // If code has no tail, we want to prevent it from resolving to `()` if it
                    // actually resolved to `!`.
                    // At the same time we don't try to be overly smart and catch cases such as
                    // `return false; 42` (e.g. block de facto resolves to `!` but has a tail),
                    // since it's still valid rust and it typechecks. Compiler will warn user
                    // about dead code anyway.
                    if tail.is_none() && self.tailless_block_final_statement_diverges(&statements) {
                        self.pass.inference.set_expr_infer_ty(expr, InferTy::Never);
                    } else {
                        self.pass.inference.set_expr_block_from_tail(expr, tail);
                    }
                }
                ExprKind::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    self.pass
                        .inference
                        .set_expr_if_from_branches(expr, then_branch, else_branch);
                }
                ExprKind::Match { arms, .. } => {
                    self.pass.inference.set_expr_match_from_arms(
                        expr,
                        arms.into_iter().filter_map(|arm| arm.expr),
                    );
                }
                _ => {}
            }
        }
    }

    /// Treat tail-less `{ return value; }`-style blocks as diverging.
    fn tailless_block_final_statement_diverges(&self, statements: &[StmtId]) -> bool {
        let Some(statement) = statements.last() else {
            return false;
        };
        let StmtKind::Expr { expr, .. } = self.pass.body.statement_unchecked(*statement).kind
        else {
            return false;
        };

        match self.pass.body.expr_unchecked(expr).kind {
            ExprKind::Break { value: Some(_), .. } => return false,
            ExprKind::Wrapper {
                kind: ExprWrapperKind::Return,
                ..
            }
            | ExprKind::Break { value: None, .. }
            | ExprKind::Continue { .. }
            | ExprKind::Yeet { .. }
            | ExprKind::Become { .. } => return true,
            _ => {}
        }

        matches!(
            self.pass.inference.root_resolved_expr_ty(expr),
            InferTy::Never
        )
    }

    /// Use one selected call target to push parameter evidence into written args.
    ///
    /// `take_user(value)` makes `value` expect `User`; `id(user)` lets `T` become `User`.
    fn constrain_call_target_argument_expected_types(
        &mut self,
        call: ExprId,
        args: &[ExprId],
    ) -> Result<(), PackageStoreError> {
        // Concrete parameter types can immediately constrain literals and transparent shapes.
        let concrete_expectations =
            BodyCallInference::new(self.pass.context()).argument_expected_tys(call, args)?;
        for (arg, expected_ty) in concrete_expectations {
            self.constrain_expr_with_expected(arg, &expected_ty);
        }

        // Build a fresh field-split context after concrete constraints. Keeping the first context
        // alive would immutably borrow the pass while we mutate the inference facts.
        let context = self.pass.providers.context(self.pass.body);
        // Generic parameter evidence needs the inference view so shared `?T` slots stay linked.
        BodyCallInference::new(context).constrain_function_generic_arguments(
            &mut self.pass.inference,
            call,
            args,
        )
    }

    /// Use inline `impl Fn*` syntax to solve matching closure arguments.
    ///
    /// Parameter expectations run earlier because they can affect method lookup inside the
    /// closure body. This final inference hook exists for the inline `impl FnOnce(...) -> R`
    /// shape: it can preserve shared slots such as `?R` and then reuse the callable-goal solver.
    fn solve_direct_callable_closure_arguments(
        &mut self,
        call: ExprId,
        args: &[ExprId],
    ) -> Result<(), PackageStoreError> {
        let context = self.pass.providers.context(self.pass.body);
        BodyCallInference::new(context).solve_direct_callable_closure_arguments(
            &mut self.pass.inference,
            call,
            args,
        )
    }

    fn solve_call_target_generic_trait_obligations(
        &mut self,
        call: ExprId,
        args: &[ExprId],
        receiver: Option<ExprId>,
    ) -> Result<(), PackageStoreError> {
        let context = self.pass.providers.context(self.pass.body);
        BodyCallInference::new(context).solve_generic_trait_obligations(
            &mut self.pass.inference,
            call,
            args,
            receiver,
        )
    }

    fn project_selected_trait_associated_return_type(
        &mut self,
        call: ExprId,
        args: &[ExprId],
        receiver: Option<ExprId>,
    ) -> Result<(), PackageStoreError> {
        let context = self.pass.providers.context(self.pass.body);
        BodyCallInference::new(context).project_selected_trait_associated_return_type(
            &mut self.pass.inference,
            call,
            args,
            receiver,
        )
    }

    /// Visit all places that can provide expected types to already-created inference slots.
    fn constrain_expected_types(&mut self) -> Result<(), PackageStoreError> {
        for statement_idx in 0..self.pass.body.statements().len() {
            self.constrain_statement_expected_types(StmtId(statement_idx))?;
        }
        for expr_idx in 0..self.pass.body.exprs().len() {
            self.constrain_expr_expected_types(ExprId(expr_idx))?;
        }
        self.constrain_function_return_expected_types()?;

        // Some constraints solve binding slots that path expressions copied before inference ran.
        // Refresh dependent facts once so finalization sees those solved locals through later reads.
        self.refresh_inference_dependent_expr_facts()?;

        Ok(())
    }

    /// Route statement-level evidence, currently `let name: T = initializer`.
    fn constrain_statement_expected_types(
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
            } => self.constrain_let_annotation_initializer(scope, pat, annotation, initializer),
            StmtKind::Let { .. }
            | StmtKind::Expr { .. }
            | StmtKind::Item { .. }
            | StmtKind::ItemIgnored => Ok(()),
        }
    }

    /// Constrain an initializer from its explicit statement annotation.
    ///
    /// This intentionally only links the annotation to the initializer root expression. Nested
    /// expected-type propagation belongs in expression kind-specific rules.
    fn constrain_let_annotation_initializer(
        &mut self,
        scope: ScopeId,
        pat: PatId,
        annotation: TypeRef,
        initializer: ExprId,
    ) -> Result<(), PackageStoreError> {
        let expected_ty = self
            .pass
            .context()
            .type_refs(TypeRefUseSite::Scope(scope))
            .resolve(&annotation)?;
        let (expected_infer_ty, used_annotation_vars) = self
            .pass
            .inference
            .instantiate_written_infer_ty(&annotation, &expected_ty);

        // `let value: Vec<_> = make_vec();` needs the written `_` to become a real inference
        // slot before call obligations run, otherwise `Vec<unknown>` cannot absorb trait evidence.
        if used_annotation_vars {
            self.pass
                .inference
                .constrain_expr_infer_ty(initializer, &expected_infer_ty);
            self.constrain_single_binding_annotation(pat, &expected_infer_ty);
        }

        self.constrain_expr_with_expected(initializer, &expected_ty);

        Ok(())
    }

    /// Attach annotation holes to a single local binding.
    ///
    /// This processes `let value: Vec<_> = ...`, where the whole annotation belongs to one
    /// binding. It intentionally does not process destructuring annotations such as
    /// `let (left, right): (Vec<_>, Vec<_>) = ...`; those need inference-aware pattern
    /// propagation rather than assigning the whole tuple type to one binding.
    fn constrain_single_binding_annotation(&mut self, pat: PatId, expected_ty: &InferTy) {
        let Some(pat_data) = self.pass.body.pat(pat).cloned() else {
            return;
        };
        let PatKind::Binding {
            binding: Some(binding),
            ..
        } = pat_data.kind
        else {
            return;
        };

        self.pass
            .inference
            .set_binding_infer_ty(binding, expected_ty.clone());
    }

    /// Route expression-level evidence from calls, method calls, record fields, and assignments.
    fn constrain_expr_expected_types(&mut self, expr: ExprId) -> Result<(), PackageStoreError> {
        let kind = self.pass.body.expr_unchecked(expr).kind.clone();
        match kind {
            ExprKind::Call {
                callee: Some(callee),
                args,
            } => {
                self.constrain_call_target_argument_expected_types(expr, &args)?;
                self.solve_direct_callable_closure_arguments(expr, &args)?;
                self.project_selected_trait_associated_return_type(expr, &args, None)?;
                self.solve_call_target_generic_trait_obligations(expr, &args, None)?;
                self.constrain_enum_variant_payload_expected_types(expr, callee, args)
            }
            ExprKind::MethodCall {
                receiver: Some(receiver),
                args,
                ..
            } => {
                self.constrain_call_target_argument_expected_types(expr, &args)?;
                self.solve_direct_callable_closure_arguments(expr, &args)?;

                let context = self.pass.providers.context(self.pass.body);
                BodyCallInference::new(context).constrain_selected_method_receiver_and_arguments(
                    &mut self.pass.inference,
                    expr,
                    receiver,
                    &args,
                )?;
                self.project_selected_trait_associated_return_type(expr, &args, Some(receiver))?;
                self.solve_call_target_generic_trait_obligations(expr, &args, Some(receiver))
            }
            ExprKind::MethodCall { args, .. } => {
                self.constrain_call_target_argument_expected_types(expr, &args)?;
                self.solve_direct_callable_closure_arguments(expr, &args)?;
                self.project_selected_trait_associated_return_type(expr, &args, None)?;
                self.solve_call_target_generic_trait_obligations(expr, &args, None)
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
        let BodyResolution::Binding(binding) = self.pass.body.expr_resolution(target) else {
            return;
        };

        let target_ty = self.pass.inference.root_resolved_expr_ty(target);
        let value_ty = self.pass.inference.root_resolved_expr_ty(value);
        self.pass
            .inference
            .constrain_infer_tys(&target_ty, &value_ty);
        self.pass
            .inference
            .set_binding_infer_ty(*binding, target_ty);
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
        let BodyResolution::Declarations(declarations) = self.pass.body.expr_resolution(callee)
        else {
            return Ok(());
        };
        let (variant_ref, enum_ty) = if let Some(DeclarationRef::EnumVariant(variant_ref)) =
            declarations.as_one()
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
            self.constrain_enum_variant_payload_infer_ty(
                call,
                arg,
                &enum_ty,
                variant_ref,
                index,
                &expected_ty,
            )?;
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
        enum_ty: &NominalTy,
        variant_ref: EnumVariantRef,
        field_index: usize,
        resolved_field_ty: &Ty,
    ) -> Result<(), PackageStoreError> {
        let item_query = self.pass.context().item_query();
        let Some(variant_data) = item_query.enum_variant_data(variant_ref)? else {
            return Ok(());
        };
        let Some(field_ty) =
            Self::tuple_variant_field_ty_ref(&variant_data.variant.fields, field_index).cloned()
        else {
            return Ok(());
        };
        let Some(generics) = item_query
            .generic_params_for_type_def(enum_ty.def)?
            .cloned()
        else {
            return Ok(());
        };

        let subst = self
            .infer_subst_for_nominal_expr(call, enum_ty, &generics)
            .unwrap_or_default();

        let expected_ty =
            InferTypeRefProjector::new(&subst).ty_from_type_ref(&field_ty, resolved_field_ty);
        self.pass
            .inference
            .constrain_expr_infer_ty(arg, &expected_ty);
        Ok(())
    }

    /// Bind generic params from a nominal expression result such as `Pair<?T>`.
    fn infer_subst_for_nominal_expr(
        &mut self,
        expr: ExprId,
        nominal_ty: &NominalTy,
        generics: &GenericParams,
    ) -> Option<InferTypeSubst> {
        let infer_ty = self.pass.inference.expr_ty(expr);
        let infer_args = match infer_ty {
            InferTy::Nominal(infer_nominal_ty) | InferTy::SelfTy(infer_nominal_ty)
                if infer_nominal_ty.def == nominal_ty.def =>
            {
                infer_nominal_ty.args
            }
            _ => return None,
        };

        let mut subst = InferTypeSubst::new();
        self.pass
            .inference
            .bind_type_params_from_infer_args(&mut subst, generics, &infer_args);
        Some(subst)
    }

    /// Find a variant field type syntax by the call-site payload position.
    fn tuple_variant_field_ty_ref(fields: &FieldList, index: usize) -> Option<&TypeRef> {
        match fields {
            FieldList::Tuple(fields) => fields.get(index).map(|field| &field.ty),
            FieldList::Named(_) | FieldList::Unit => None,
        }
    }

    /// Use record type and field key to push declared field types into initializers.
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
            if let Some(field_ty_ref) = target.ty_ref().cloned() {
                self.constrain_record_field_initializer_infer_ty(
                    record,
                    value,
                    &record_ty,
                    &field_ty_ref,
                    &expected_ty,
                )?;
            }
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
        record_ty: &NominalTy,
        field_ty: &TypeRef,
        resolved_field_ty: &Ty,
    ) -> Result<(), PackageStoreError> {
        let Some(generics) = self
            .pass
            .context()
            .item_query()
            .generic_params_for_type_def(record_ty.def)?
            .cloned()
        else {
            return Ok(());
        };
        let Some(subst) = self.infer_subst_for_nominal_expr(record, record_ty, &generics) else {
            return Ok(());
        };

        let expected_ty =
            InferTypeRefProjector::new(&subst).ty_from_type_ref(field_ty, resolved_field_ty);
        self.pass
            .inference
            .constrain_expr_infer_ty(value, &expected_ty);
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
        let Some(function) = self.pass.body.function_owner() else {
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
    fn array_len_matches_count(expected_len: &Option<String>, element_count: usize) -> bool {
        expected_len
            .as_deref()
            .is_none_or(|len| len == element_count.to_string())
    }

    /// Accept missing array length, otherwise match it against repeat syntax text.
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
        // Persist the inference view back into Body IR so downstream queries see ordinary `Ty`
        // facts. Unsolved numeric variables become defaults; conflicts become `<unknown>`.
        for expr_idx in 0..self.pass.body.exprs().len() {
            let expr = ExprId(expr_idx);
            let ty = self.pass.inference.finalize_expr_ty(expr);
            self.pass.body.set_expr_ty(expr, ty);
        }
        for binding_idx in 0..self.pass.body.bindings().len() {
            let binding = BindingId(binding_idx);
            let ty = self.pass.inference.finalize_binding_ty(binding);
            self.pass.body.set_binding_ty(binding, ty);
        }
    }
}
