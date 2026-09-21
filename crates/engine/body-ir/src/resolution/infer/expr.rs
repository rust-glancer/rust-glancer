//! Recursive inference follows source ownership: a block introduces statements, each expression
//! infers its children, and consumers read their live types from inference state.
//!
//! An expectation flows down into children where the syntax gives us a matching shape, such as
//! tuple fields or call arguments. Their stored slots also connect the result back to its inputs:
//! a later constraint on the result can still reach a child whose syntax has already been visited.

use anyhow::Context as _;
use rg_def_map::DefMapSource;
use rg_ir_model::{ExprId, FieldKey, StmtId, identity::DeclarationRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::Ty;

use super::{InferenceContext, fulfill::DeferredKind};
use crate::body::{ExprAssignOp, ExprKind, ExprWrapperKind, StmtKind, facts::BodyResolution};

impl<'query, D, I> InferenceContext<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    /// Infer one expression against its expectation and store its type in inference state.
    /// Shared variables carry later evidence without revisiting this subtree.
    pub(super) fn infer_expr(&mut self, expr: ExprId, expected: &Ty) -> anyhow::Result<()> {
        #[cfg(test)]
        self.before_expression();
        rg_std::check_cancel!(self.context, "expression resolution");
        // Keep generated or expanded syntax from consuming an unbounded inference stack.
        // An interrupted subtree stays unknown while the rest of the body can keep its results.
        if self.depth == 256 {
            self.inference_exhausted = true;
            crate::profile::metric::RECURSION_EXHAUSTIONS.inc();
            return Ok(());
        }
        self.depth += 1;
        crate::profile::metric::EXPRESSION_VISITS.inc();
        let result = self.infer_expr_inner(expr, expected);
        self.depth -= 1;
        result
    }

    fn infer_expr_inner(&mut self, expr: ExprId, expected: &Ty) -> anyhow::Result<()> {
        // Syntax belongs to the immutable body, independently of the mutable inference state.
        let body = self.body;
        match body.expr_unchecked(expr).kind {
            ExprKind::Block {
                ref statements,
                tail,
                ..
            } => {
                for statement in statements {
                    self.infer_statement(*statement)
                        .context("infer block statement")?;
                }
                self.infer_optional(tail, expected)
                    .context("infer block tail")?;
                // Statements and the tail can refine locals used by earlier pending operations.
                // Let those operations use the new evidence before reading the block's result.
                self.fulfill_pending().context("complete block inference")?;
                if let Some(tail) = tail {
                    let ty = self.inference.expr_slot(tail);
                    self.inference.set_expr_fact_allowing_weak_slot(expr, ty);
                } else {
                    // A tailless block is normally unit, but its final statement can make it
                    // diverge. Recognize the direct diverging forms as well as an inferred `!`.
                    let diverges = statements.last().is_some_and(|statement| {
                        let StmtKind::Expr { expr, .. } = body.statement_unchecked(*statement).kind
                        else {
                            return false;
                        };
                        match body.expr_unchecked(expr).kind {
                            ExprKind::Break { value: Some(_), .. } => false,
                            ExprKind::Wrapper {
                                kind: ExprWrapperKind::Return,
                                ..
                            }
                            | ExprKind::Break { value: None, .. }
                            | ExprKind::Continue { .. }
                            | ExprKind::Yeet { .. }
                            | ExprKind::Become { .. } => true,
                            _ => matches!(self.inference.root_resolved_expr_ty(expr), Ty::Never),
                        }
                    });
                    self.inference
                        .set_expr_ty(expr, if diverges { Ty::Never } else { Ty::Unit });
                }
            }
            ExprKind::Call { callee, ref args } => {
                self.infer_optional(callee, &Ty::Unknown)
                    .context("infer optional expression")?;
                if let Some(callee) = callee {
                    let callee_ty = self.inference.root_resolved_expr_ty(callee);
                    if matches!(callee_ty, Ty::Adt(_)) {
                        self.inference.set_expr_ty(expr, callee_ty);
                    }
                }
                let variant =
                    callee.and_then(|callee| match self.inference.expr_resolution(callee) {
                        BodyResolution::Declarations(declarations) => match declarations.as_one() {
                            Some(DeclarationRef::EnumVariant(variant)) => Some(*variant),
                            _ => None,
                        },
                        _ => None,
                    });
                if let Some(variant) = variant {
                    // A tuple variant gets argument expectations from its enum's fields. Give
                    // omitted enum arguments live slots first, so `Some(value)` and an expected
                    // `Option<User>` can pass evidence through the same element type.
                    let ty = self.inference.expr_ty(expr);
                    self.inference.instantiate_expr_nested_unknown_ty(expr, &ty);
                    self.inference.constrain_expr_ty(expr, expected);
                    let ty = self.inference.root_resolved_expr_ty(expr);
                    for (index, arg) in args.iter().enumerate() {
                        let expected = match ty.as_adts() {
                            [nominal] => self
                                .context
                                .fields()
                                .enum_variant_field_ty(nominal, variant, &FieldKey::Tuple(index))
                                .context("resolve variant field type")?
                                .unwrap_or(Ty::Unknown),
                            _ => Ty::Unknown,
                        };
                        self.infer_expr(*arg, &expected)
                            .context("infer variant argument")?;
                    }
                } else {
                    self.infer_call(expr, args, None, expected)
                        .context("infer call")?;
                }
            }
            ExprKind::MethodCall {
                receiver, ref args, ..
            } => {
                self.infer_optional(receiver, &Ty::Unknown)
                    .context("infer optional expression")?;
                self.method_calls.push(expr);
                self.infer_call(expr, args, receiver, expected)
                    .context("infer call")?;
            }
            ExprKind::Tuple { ref fields } => {
                let expected = self.inference.root_resolved_ty(expected);
                // Retain the children's slots in the tuple. A later expectation for the whole
                // tuple must still be able to constrain a field that is unknown here.
                let mut field_tys = Vec::with_capacity(fields.len());
                for (index, field) in fields.iter().enumerate() {
                    let expected = match &expected {
                        Ty::Tuple(types) if types.len() == fields.len() => &types[index],
                        _ => &Ty::Unknown,
                    };
                    self.infer_expr(*field, expected)
                        .context("infer tuple field")?;
                    field_tys.push(self.inference.expr_slot(*field));
                }
                self.inference
                    .set_expr_fact_allowing_weak_slot(expr, Ty::tuple(field_tys));
            }
            ExprKind::Array { ref elements } => {
                let expected = self.inference.root_resolved_ty(expected);
                let element_ty = match &expected {
                    Ty::Array { inner, len }
                        if matches!(
                            len,
                            rg_ty::ConstValue::Unknown | rg_ty::ConstValue::Param(_)
                        ) || *len == rg_ty::ConstValue::Scalar(elements.len() as u128) =>
                    {
                        inner.as_ref()
                    }
                    _ => &Ty::Unknown,
                };
                // Every element shares this destination. Expected types and later sibling
                // evidence constrain the same live slots, without revisiting earlier elements.
                if !elements.is_empty() {
                    let shared_element = self.inference.table_mut().new_type_var();
                    for element in elements {
                        self.infer_expr(*element, element_ty)
                            .context("infer array element")?;
                        let ty = self.inference.expr_slot(*element);
                        self.inference.constrain_infer_tys(&shared_element, &ty);
                    }
                    self.inference.set_expr_fact_allowing_weak_slot(
                        expr,
                        Ty::Array {
                            inner: Box::new(shared_element),
                            len: rg_ty::ConstValue::Scalar(elements.len() as u128),
                        },
                    );
                }
            }
            ExprKind::RepeatArray {
                initializer,
                repeat,
                ref len_text,
            } => {
                let expected = self.inference.root_resolved_ty(expected);
                let element_ty = match &expected {
                    Ty::Array { inner, .. } => inner.as_ref(),
                    _ => &Ty::Unknown,
                };
                self.infer_optional(initializer, element_ty)
                    .context("infer array initializer")?;
                self.infer_optional(repeat, &Ty::Unknown)
                    .context("infer array length")?;
                if let Some(initializer) = initializer {
                    let ty = self.inference.expr_slot(initializer);
                    self.inference.set_expr_fact_allowing_weak_slot(
                        expr,
                        Ty::Array {
                            inner: Box::new(ty),
                            len: len_text
                                .as_deref()
                                .map(rg_ty::ConstValue::from_syntax)
                                .unwrap_or(rg_ty::ConstValue::Unknown),
                        },
                    );
                }
            }
            ExprKind::Index { base, index } => {
                self.infer_optional(base, &Ty::Unknown)
                    .context("infer optional expression")?;
                self.infer_optional(index, &Ty::Unknown)
                    .context("infer optional expression")?;
                self.inference.expr_slot(expr);
                if base.is_some() {
                    self.run_or_defer(DeferredKind::Member { expr })
                        .context("register pending inference")?;
                }
            }
            ExprKind::Field { base, .. } => {
                self.infer_optional(base, &Ty::Unknown)
                    .context("infer optional expression")?;
                self.inference.expr_slot(expr);
                if base.is_some() {
                    self.run_or_defer(DeferredKind::Member { expr })
                        .context("register pending inference")?;
                }
            }
            ExprKind::Range { start, end, .. } => {
                self.infer_optional(start, &Ty::Unknown)
                    .context("infer optional expression")?;
                self.infer_optional(end, &Ty::Unknown)
                    .context("infer optional expression")?;
            }
            ExprKind::Cast {
                expr: inner,
                ref ty,
            } => {
                self.infer_optional(inner, &Ty::Unknown)
                    .context("infer optional expression")?;
                if let Some(ty) = ty {
                    let ty = self
                        .context
                        .type_refs(self.body.expr_unchecked(expr).scope)
                        .resolve(ty)
                        .context("resolve cast type")?;
                    self.inference.set_expr_ty(expr, ty);
                }
            }
            ExprKind::Unary { expr: inner, .. } => {
                self.infer_optional(inner, &Ty::Unknown)
                    .context("infer optional expression")?;
                if inner.is_some() {
                    self.inference.expr_slot(expr);
                    self.run_or_defer(DeferredKind::Operator { expr })
                        .context("register pending inference")?;
                }
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                self.infer_optional(lhs, &Ty::Unknown)
                    .context("infer optional expression")?;
                self.infer_optional(rhs, &Ty::Unknown)
                    .context("infer optional expression")?;
                if lhs.is_some() && rhs.is_some() {
                    self.inference.expr_slot(expr);
                    self.run_or_defer(DeferredKind::Operator { expr })
                        .context("register pending inference")?;
                }
            }
            ExprKind::Assign { target, op, value } => {
                self.infer_optional(target, &Ty::Unknown)
                    .context("infer optional expression")?;
                let ty = match (target, op) {
                    (Some(target), Some(ExprAssignOp::Assign))
                        if matches!(
                            self.inference.expr_resolution(target),
                            BodyResolution::Binding(_)
                        ) =>
                    {
                        self.inference.expr_slot(target)
                    }
                    _ => Ty::Unknown,
                };
                self.infer_optional(value, &Ty::Unknown)
                    .context("infer optional expression")?;
                if let Some(value) = value {
                    self.coerce_expr_ty(value, &ty);
                }
                self.inference.set_expr_ty(expr, Ty::Unit);
            }
            ExprKind::Match {
                scrutinee,
                ref arms,
            } => {
                self.infer_optional(scrutinee, &Ty::Unknown)
                    .context("infer optional expression")?;
                let scrutinee = scrutinee
                    .map(|expr| self.inference.expr_slot(expr))
                    .unwrap_or(Ty::Unknown);
                for arm in arms {
                    if let Some(pat) = arm.pat {
                        self.infer_pattern(pat, &scrutinee)
                            .context("infer match pattern")?;
                    }
                    self.infer_optional(arm.guard, &Ty::Unknown)
                        .context("infer optional expression")?;
                    self.infer_optional(arm.expr, expected)
                        .context("infer optional expression")?;
                }
                self.infer_branch_result(expr, arms.iter().filter_map(|arm| arm.expr))
                    .context("infer match result")?;
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.infer_optional(condition, &Ty::Unknown)
                    .context("infer optional expression")?;
                let branch_expected = if else_branch.is_some() {
                    expected
                } else {
                    &Ty::Unknown
                };
                self.infer_optional(then_branch, branch_expected)
                    .context("infer optional expression")?;
                self.infer_optional(else_branch, branch_expected)
                    .context("infer optional expression")?;
                if let Some(else_branch) = else_branch {
                    self.infer_branch_result(expr, then_branch.into_iter().chain([else_branch]))
                        .context("infer if result")?;
                } else {
                    self.inference.set_expr_ty(expr, Ty::Unit);
                }
            }
            ExprKind::Let {
                pat, initializer, ..
            } => {
                self.infer_optional(initializer, &Ty::Unknown)
                    .context("infer optional expression")?;
                let ty = initializer
                    .map(|expr| self.inference.expr_slot(expr))
                    .unwrap_or(Ty::Unknown);
                if let Some(pat) = pat {
                    self.infer_pattern(pat, &ty).context("infer let pattern")?;
                }
                self.inference
                    .set_expr_ty(expr, Ty::Primitive(rg_ty::PrimitiveTy::Bool));
            }
            ExprKind::Closure {
                scope,
                ref params,
                ref ret_ty,
                body,
                ..
            } => {
                self.infer_closure(expr, scope, params, ret_ty.as_ref(), body)
                    .context("infer closure")?;
            }
            ExprKind::Loop { body, .. } => {
                self.infer_optional(body, &Ty::Unknown)
                    .context("infer optional expression")?;
            }
            ExprKind::While {
                condition, body, ..
            } => {
                self.infer_optional(condition, &Ty::Unknown)
                    .context("infer optional expression")?;
                self.infer_optional(body, &Ty::Unknown)
                    .context("infer optional expression")?;
                self.inference.set_expr_ty(expr, Ty::Unit);
            }
            ExprKind::For {
                pat,
                iterable,
                body,
                ..
            } => {
                self.infer_optional(iterable, &Ty::Unknown)
                    .context("infer optional expression")?;
                if let (Some(pat), Some(iterable)) = (pat, iterable) {
                    // The loop pattern can use its item slot before `IntoIterator::Item` is
                    // known. A later projection fills the same slot, including its binding uses.
                    let item = self.inference.table_mut().new_type_var();
                    self.run_or_defer(DeferredKind::IteratorItem {
                        iterable,
                        item: item.clone(),
                    })
                    .context("register pending inference")?;
                    self.infer_pattern(pat, &item)
                        .context("infer iterator pattern")?;
                }
                self.infer_optional(body, &Ty::Unknown)
                    .context("infer optional expression")?;
                self.inference.set_expr_ty(expr, Ty::Unit);
            }
            ExprKind::Break { value, .. }
            | ExprKind::Yield { value }
            | ExprKind::Yeet { value }
            | ExprKind::Become { value } => {
                self.infer_optional(value, &Ty::Unknown)
                    .context("infer optional expression")?;
                if !matches!(self.body.expr_unchecked(expr).kind, ExprKind::Yield { .. }) {
                    self.inference.set_expr_ty(expr, Ty::Never);
                }
            }
            ExprKind::Record {
                ref path,
                ref fields,
                ref spread,
                ..
            } => {
                let (resolution, ty) = match path.as_ref() {
                    Some(path) => self
                        .context
                        .value_paths()
                        .resolve_record_expr_path(self.body.expr_unchecked(expr).scope, path)
                        .context("resolve record path")?,
                    None => (BodyResolution::Unknown, Ty::Unknown),
                };
                self.inference.set_expr_facts(expr, resolution, ty);
                // Path lookup supplies the record's identity and written generic arguments.
                // Make omitted arguments inferable before deriving expectations for the fields.
                let ty = self.inference.expr_ty(expr);
                self.inference.instantiate_expr_nested_unknown_ty(expr, &ty);
                self.inference.constrain_expr_ty(expr, expected);
                let ty = self.inference.root_resolved_expr_ty(expr);
                for field in fields {
                    let expected = match ty.as_adts() {
                        [nominal] => self
                            .context
                            .fields()
                            .declared(nominal, &field.key)
                            .context("resolve record field")?
                            .and_then(|field| field.ty().cloned())
                            .unwrap_or(Ty::Unknown),
                        _ => Ty::Unknown,
                    };
                    self.infer_optional(field.value, &expected)
                        .context("infer optional expression")?;
                }
                self.infer_optional(spread.as_ref().and_then(|spread| spread.expr), &Ty::Unknown)
                    .context("infer optional expression")?;
            }
            ExprKind::Wrapper { kind, inner } => {
                let resolved_expected = self.inference.root_resolved_ty(expected);
                let inner_expected = match (&kind, &resolved_expected) {
                    (ExprWrapperKind::Paren | ExprWrapperKind::Await, _) => expected.clone(),
                    (
                        ExprWrapperKind::Ref { mutability },
                        Ty::Reference {
                            mutability: expected_mutability,
                            inner,
                            ..
                        },
                    ) if mutability == expected_mutability => inner.as_ref().clone(),
                    (ExprWrapperKind::Return, _) => self.return_ty.clone(),
                    _ => Ty::Unknown,
                };
                self.infer_optional(inner, &inner_expected)
                    .context("infer wrapped expression")?;
                if matches!(kind, ExprWrapperKind::Try) && inner.is_some() {
                    self.inference.expr_slot(expr);
                    self.run_or_defer(DeferredKind::TryOutput { expr })
                        .context("register pending inference")?;
                } else if let Some(inner) = inner {
                    let inner_ty = self.inference.expr_slot(inner);
                    // Await is shallow: async functions expose their declared result here.
                    // TODO: Model arbitrary Future::Output when inference coverage expands.
                    let ty = match kind {
                        ExprWrapperKind::Paren | ExprWrapperKind::Await => inner_ty,
                        ExprWrapperKind::Ref { mutability } => Ty::reference(mutability, inner_ty),
                        ExprWrapperKind::Return => Ty::Never,
                        ExprWrapperKind::Try => unreachable!("try operands are deferred above"),
                    };
                    self.inference.set_expr_fact_allowing_weak_slot(expr, ty);
                    if matches!(kind, ExprWrapperKind::Paren) {
                        self.inference.set_expr_resolution(
                            expr,
                            self.inference.expr_resolution(inner).clone(),
                        );
                    }
                } else if matches!(kind, ExprWrapperKind::Return) {
                    self.inference.set_expr_ty(expr, Ty::Never);
                }
            }
            ExprKind::Unknown { ref children } => {
                for child in children {
                    self.infer_expr(*child, &Ty::Unknown)
                        .context("infer unmodeled expression child")?;
                }
            }
            ExprKind::Path { ref path } => {
                let (resolution, ty) = self
                    .context
                    .value_paths()
                    .resolve_body_path_expr(expr, path)
                    .context("resolve body path")?;
                if let BodyResolution::Binding(binding) = resolution {
                    self.inference.set_expr_resolution(expr, resolution);
                    self.inference.set_expr_from_binding(expr, binding);
                } else {
                    self.inference.set_expr_facts(expr, resolution, ty);
                }
            }
            // Unsuffixed numbers stay inferable until completion: a later use may require `u64`
            // or `f32`, even when the literal had no expectation at its introduction site.
            ExprKind::Literal { kind } => match kind {
                crate::body::LiteralKind::Int { primitive_ty: None } => {
                    let ty = self.inference.table_mut().new_integer_var();
                    self.inference.set_expr_infer_ty(expr, ty);
                }
                crate::body::LiteralKind::Float { primitive_ty: None } => {
                    let ty = self.inference.table_mut().new_float_var();
                    self.inference.set_expr_infer_ty(expr, ty);
                }
                _ => self
                    .inference
                    .set_expr_ty(expr, rg_ty::ty_for_literal(kind)),
            },
            ExprKind::BuiltinMacro { kind } => {
                let ty = self
                    .builtin_macro_ty(expr, kind)
                    .context("infer builtin macro")?;
                self.inference.set_expr_ty(expr, ty);
            }
            ExprKind::Continue { .. } => self.inference.set_expr_ty(expr, Ty::Never),
            ExprKind::Underscore => {}
        }
        // A block with a diverging tail can satisfy a concrete expected result, while the tail
        // expression itself keeps `!`. This is the small block coercion supported by body facts.
        if matches!(
            self.body.expr_unchecked(expr).kind,
            ExprKind::Block { tail: Some(_), .. }
        ) && matches!(self.inference.root_resolved_expr_ty(expr), Ty::Never)
            && !matches!(
                self.inference.root_resolved_ty(expected),
                Ty::Unknown | Ty::Never | Ty::InferVar { .. }
            )
        {
            self.inference
                .set_expr_fact_allowing_weak_slot(expr, expected.clone());
        } else {
            self.coerce_expr_ty(expr, expected);
        }
        Ok(())
    }

    pub(super) fn infer_optional(
        &mut self,
        expr: Option<ExprId>,
        expected: &Ty,
    ) -> anyhow::Result<()> {
        match expr {
            Some(expr) => self
                .infer_expr(expr, expected)
                .context("infer child expression"),
            None => Ok(()),
        }
    }

    /// A branch supplies a value by coercion, so its type need not equal the shared result: a
    /// deferred call can still reveal `!`. Retain those relationships until its producer settles.
    fn infer_branch_result(
        &mut self,
        expr: ExprId,
        result_exprs: impl Iterator<Item = ExprId>,
    ) -> anyhow::Result<()> {
        let branches = result_exprs
            .map(|branch| self.inference.expr_slot(branch))
            .collect();
        self.run_or_defer(DeferredKind::BranchResult { expr, branches })
            .context("coerce branch results")
    }

    fn infer_statement(&mut self, statement: StmtId) -> anyhow::Result<()> {
        let body = self.body;
        match body.statement_unchecked(statement).kind {
            StmtKind::Let {
                scope,
                pat,
                ref annotation,
                initializer,
                else_branch,
                ..
            } => {
                let expected = match annotation {
                    Some(annotation) => self
                        .context
                        .type_refs(scope)
                        .resolve_with_inference(annotation, self.inference.table_mut())
                        .context("resolve let annotation")?,
                    None => Ty::Unknown,
                };
                self.infer_optional(initializer, &expected)
                    .context("infer optional expression")?;
                // Written types remain the binding's contract even when incomplete source has
                // an incompatible initializer. Inferred bindings instead share the producer slot.
                let ty = if matches!(expected, Ty::Unknown) {
                    initializer
                        .map(|expr| self.inference.expr_slot(expr))
                        .unwrap_or(Ty::Unknown)
                } else {
                    expected
                };
                if let Some(pat) = pat {
                    self.infer_pattern(pat, &ty)
                        .context("infer binding pattern")?;
                }
                self.infer_optional(else_branch, &Ty::Unknown)
                    .context("infer optional expression")?;
            }
            StmtKind::Expr { expr, .. } => {
                self.infer_expr(expr, &Ty::Unknown)
                    .context("infer statement expression")?;
            }
            StmtKind::Item { .. } | StmtKind::ItemIgnored => {}
        }
        Ok(())
    }
}
