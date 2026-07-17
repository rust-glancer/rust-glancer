//! Expression resolution for the body-resolution fixed-point pass.
//!
//! This module owns expression-shaped traversal and the type/resolution relationships derived
//! from expressions. The parent pass drives ordering and the shared fixed point.

use rg_def_map::DefMapSource;
use rg_ir_model::{
    BodyPath, DefMapRef, ExprId, GenericDefRef, Path, ScopeId, StmtId, TypeDefRef,
    TypePathResolution, identity::DeclarationRef, items::FieldKey,
};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::ExpectedUnique;
use rg_ty::{
    AdtTy, AutoderefMode, ExpectedTyExt, GenericArgs, PrimitiveTy, Substitution, Ty, ty_for_literal,
};

use crate::{
    ExprUnaryOp,
    ir::resolved::BodyResolution,
    ir::{ExprKind, ExprWrapperKind, LiteralKind, StmtKind},
};

use super::{body::BodyResolutionPass, builtin_macro::BuiltinMacroExprTypeMapper};

pub(super) struct ExprResolutionPass<'pass, 'query, 'body, D, I> {
    pass: &'pass mut BodyResolutionPass<'query, 'body, D, I>,
}

impl<'pass, 'query, 'body, D, I> ExprResolutionPass<'pass, 'query, 'body, D, I> {
    pub(super) fn new(pass: &'pass mut BodyResolutionPass<'query, 'body, D, I>) -> Self {
        Self { pass }
    }
}

impl<'pass, 'query, 'body, D, I> ExprResolutionPass<'pass, 'query, 'body, D, I>
where
    for<'source> &'source D: DefMapSource<Error = PackageStoreError>,
    for<'source> &'source I: ItemStoreSource<'source, Error = PackageStoreError>,
{
    pub(super) fn resolve_expr(&mut self, expr: ExprId) -> Result<bool, PackageStoreError> {
        let old_resolution = self.pass.expr_resolution(expr).clone();
        let expr_data = self.pass.body.expr_unchecked(expr);
        let kind = expr_data.kind.clone();

        match kind {
            ExprKind::Path { path } => {
                let (resolution, ty) = self.resolve_body_path_expr(expr, &path)?;
                if let BodyResolution::Binding(binding) = resolution {
                    self.pass
                        .set_expr_resolution(expr, BodyResolution::Binding(binding));
                    self.pass.inference.set_expr_from_binding(expr, binding);
                } else {
                    self.pass.set_expr_facts(expr, resolution, ty);
                }
            }
            ExprKind::Call { callee, .. } => {
                if let Some(callee) = callee {
                    // Enum tuple/unit constructors carry their instantiated enum type on the
                    // callee. Function calls are instantiated by `BodyCallInference`, which owns
                    // the call's stable generic slots.
                    let callee_ty = self.pass.inference.root_resolved_expr_ty(callee);
                    if matches!(callee_ty, Ty::Adt(_)) {
                        self.pass.set_expr_ty(expr, callee_ty);
                    }
                }
            }
            ExprKind::BuiltinMacro { kind } => {
                let ty = BuiltinMacroExprTypeMapper::new(self.pass.context()).ty_for(expr, kind)?;
                self.pass.set_expr_ty(expr, ty);
            }
            ExprKind::Tuple { fields } => {
                self.pass
                    .inference
                    .set_expr_tuple_from_fields(expr, &fields);
            }
            ExprKind::Array { elements } => {
                self.pass.inference.set_expr_array_from_elements(
                    expr,
                    &elements,
                    Some(elements.len().to_string()),
                );
            }
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
            ExprKind::Index { .. } => {}
            ExprKind::Cast { ty: Some(ty), .. } => {
                let ty = self
                    .pass
                    .context()
                    .type_refs(self.pass.body.expr_unchecked(expr).scope)
                    .resolve(&ty)?;
                self.pass.set_expr_ty(expr, ty);
            }
            ExprKind::Match { arms, .. } => {
                self.pass
                    .inference
                    .set_expr_match_from_arms(expr, arms.into_iter().filter_map(|arm| arm.expr));
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
            ExprKind::Block {
                statements, tail, ..
            } => {
                // A block without a tail normally produces `()`, unless its last statement cannot
                // complete. Keep this in the block transfer so there is one owner for block type.
                if tail.is_none() && self.tailless_block_final_statement_diverges(&statements) {
                    self.pass.inference.set_expr_infer_ty(expr, Ty::Never);
                } else {
                    self.pass.inference.set_expr_block_from_tail(expr, tail);
                }
            }
            ExprKind::Field { base, field, .. } => {
                let resolution = self.resolve_field_expr(base, field.as_ref())?;
                self.pass.set_expr_resolution(expr, resolution);
            }
            ExprKind::Record { path, .. } => {
                let (resolution, ty) = match path.as_ref() {
                    Some(path) => self.resolve_record_expr_path(
                        self.pass.body.expr_unchecked(expr).scope,
                        path,
                    )?,
                    None => (BodyResolution::Unknown, Ty::Unknown),
                };
                self.pass.set_expr_facts(expr, resolution, ty);
            }
            ExprKind::MethodCall { receiver, .. } => {
                let resolution = self.resolve_method_call_expr(expr, receiver)?;
                self.pass.set_expr_resolution(expr, resolution);
            }
            ExprKind::Wrapper { kind, inner } => {
                let (resolution, ty) = self.resolve_wrapper_expr(kind, inner);
                self.pass
                    .set_expr_wrapper_facts(expr, resolution, kind, inner, ty);
            }
            ExprKind::Unary {
                op: Some(ExprUnaryOp::Deref),
                expr: Some(inner),
            } => {
                let ty = self.explicit_deref_ty(inner)?;
                self.pass.set_expr_ty(expr, ty);
            }
            ExprKind::Unary {
                op: Some(op),
                expr: Some(inner),
            } => {
                self.pass
                    .inference
                    .set_expr_unary_from_inner(expr, op, inner);
            }
            ExprKind::Binary {
                lhs: Some(lhs),
                rhs: Some(rhs),
                op: Some(op),
            } => {
                self.pass
                    .inference
                    .set_expr_binary_from_operands(expr, op, lhs, rhs);
            }
            ExprKind::Literal { kind } => match kind {
                LiteralKind::Int { primitive_ty: None } => {
                    self.pass.inference.set_expr_integer_var(expr)
                }
                LiteralKind::Float { primitive_ty: None } => {
                    self.pass.inference.set_expr_float_var(expr)
                }
                _ => self.pass.set_expr_ty(expr, ty_for_literal(kind)),
            },
            ExprKind::While { .. } | ExprKind::For { .. } => {
                self.pass.set_expr_ty(expr, Ty::Unit);
            }
            ExprKind::Assign { .. } => {
                self.pass.set_expr_ty(expr, Ty::Unit);
            }
            ExprKind::Let { .. } => {
                self.pass
                    .set_expr_ty(expr, Ty::Primitive(PrimitiveTy::Bool));
            }
            ExprKind::Break { .. } | ExprKind::Continue { .. } => {
                self.pass.set_expr_ty(expr, Ty::Never);
            }
            ExprKind::Yeet { .. } | ExprKind::Become { .. } => {
                self.pass.set_expr_ty(expr, Ty::Never);
            }
            ExprKind::Closure { .. }
            | ExprKind::Loop { .. }
            | ExprKind::Range { .. }
            | ExprKind::Cast { ty: None, .. }
            | ExprKind::Unary { .. }
            | ExprKind::Binary { .. }
            | ExprKind::Underscore
            | ExprKind::Yield { .. }
            | ExprKind::Unknown { .. } => {}
        }

        Ok(self.pass.expr_resolution(expr) != &old_resolution)
    }

    fn resolve_path_expr(
        &self,
        expr: ExprId,
        path: &Path,
    ) -> Result<(BodyResolution, Ty), PackageStoreError> {
        let expr_data = self.pass.body.expr_unchecked(expr);
        let scope = expr_data.scope;
        let visible_bindings = expr_data.visible_bindings;
        self.pass
            .context()
            .value_paths()
            .resolve_path_expr(scope, path, Some(visible_bindings))
    }

    /// Recognize the direct diverging forms that make a tailless block evaluate to `!`.
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

        matches!(self.pass.inference.root_resolved_expr_ty(expr), Ty::Never)
    }

    fn resolve_body_path_expr(
        &self,
        expr: ExprId,
        path: &BodyPath,
    ) -> Result<(BodyResolution, Ty), PackageStoreError> {
        let expr_data = self.pass.body.expr_unchecked(expr);

        if let Some(result) = self
            .pass
            .context()
            .associated_items()
            .resolve_body_path(expr_data.scope, path)?
        {
            return Ok(result);
        }

        match path.as_def_map_path() {
            Some(path) => self.resolve_path_expr(expr, &path),
            None => Ok((BodyResolution::Unknown, Ty::Unknown)),
        }
    }

    fn resolve_record_expr_path(
        &self,
        scope: ScopeId,
        path: &BodyPath,
    ) -> Result<(BodyResolution, Ty), PackageStoreError> {
        let Some(def_map_path) = path.as_def_map_path() else {
            return Ok((BodyResolution::Unknown, Ty::Unknown));
        };

        match self
            .pass
            .context()
            .type_path_query()
            .resolve_in_scope(scope, &def_map_path)?
        {
            TypePathResolution::SelfType(type_def) => {
                return Ok((
                    BodyResolution::Unknown,
                    Ty::adt(self.record_nominal_ty(scope, path, type_def)?),
                ));
            }
            TypePathResolution::TypeDef(type_def) => {
                let declaration = self.record_declaration_for_type_def(type_def)?;
                return Ok((
                    BodyResolution::Declarations([declaration].into_iter().collect()),
                    Ty::adt(self.record_nominal_ty(scope, path, type_def)?),
                ));
            }
            TypePathResolution::TypeAlias(_)
            | TypePathResolution::Trait(_)
            | TypePathResolution::Unknown => {}
        }

        // Record enum variants live in the type namespace even though they are not themselves
        // types. Resolve that identity separately so `Choice::Record { ... }` does not depend on
        // the bare-value constructor path used by tuple and unit variants.
        if let Some(variant_ref) = self
            .pass
            .context()
            .type_path_query()
            .resolve_enum_variant_in_scope(scope, &def_map_path)?
            && let Some(variant) = self
                .pass
                .context()
                .item_query()
                .enum_variant_data(variant_ref)?
        {
            return Ok((
                BodyResolution::Declarations(
                    [DeclarationRef::EnumVariant(variant_ref)]
                        .into_iter()
                        .collect(),
                ),
                Ty::adt(self.record_nominal_ty(scope, path, variant.owner)?),
            ));
        }

        self.pass
            .context()
            .value_paths()
            .resolve_nonlocal_path_expr(scope, &def_map_path)
    }

    /// Prefer the source local def for record constructors so navigation stays source-shaped.
    fn record_declaration_for_type_def(
        &self,
        type_def: TypeDefRef,
    ) -> Result<DeclarationRef, PackageStoreError> {
        if type_def.origin == DefMapRef::Body(self.pass.env.body_ref()) {
            return Ok(DeclarationRef::from(type_def));
        }

        Ok(self
            .pass
            .context()
            .item_query()
            .local_def_for_type_def(type_def)?
            .map(DeclarationRef::from)
            .unwrap_or_else(|| DeclarationRef::from(type_def)))
    }

    /// Build a record constructor result type, filling omitted type args as inferable unknowns.
    fn record_nominal_ty(
        &self,
        scope: ScopeId,
        path: &BodyPath,
        type_def: TypeDefRef,
    ) -> Result<AdtTy, PackageStoreError> {
        Ok(AdtTy {
            def: type_def,
            args: self.record_generic_args(scope, path, type_def)?,
        })
    }

    /// Preserve explicit record args, otherwise create unknown slots for type params.
    fn record_generic_args(
        &self,
        scope: ScopeId,
        path: &BodyPath,
        type_def: TypeDefRef,
    ) -> Result<GenericArgs, PackageStoreError> {
        if let Some(args) = path.last_segment_angle_args() {
            return self
                .pass
                .context()
                .type_refs(scope)
                .resolve_generic_args_for(GenericDefRef::TypeDef(type_def), args);
        }

        let generics = self
            .pass
            .context()
            .item_paths()
            .generics()
            .generics(GenericDefRef::TypeDef(type_def))?;
        Ok(Substitution::new().args_for(&generics))
    }

    fn resolve_field_expr(
        &self,
        base: Option<ExprId>,
        field: Option<&FieldKey>,
    ) -> Result<BodyResolution, PackageStoreError> {
        let (Some(base), Some(field)) = (base, field) else {
            return Ok(BodyResolution::Unknown);
        };

        let base_ty = self.pass.inference.root_resolved_expr_ty(base);
        let targets = self
            .pass
            .context()
            .fields()
            .resolve_for_ty(&base_ty, field)?;
        if targets.is_empty() {
            return Ok(BodyResolution::Unknown);
        }

        Ok(targets.resolution())
    }

    fn resolve_method_call_expr(
        &self,
        call: ExprId,
        receiver: Option<ExprId>,
    ) -> Result<BodyResolution, PackageStoreError> {
        let Some(receiver) = receiver else {
            return Ok(BodyResolution::Unknown);
        };

        let receiver_ty = self.pass.inference.root_resolved_expr_ty(receiver);
        let targets = self
            .pass
            .context()
            .calls()
            .method_targets_with_receiver_ty(call, &receiver_ty)?;
        if targets.is_empty() {
            return Ok(BodyResolution::Unknown);
        }

        Ok(targets.resolution())
    }

    fn resolve_wrapper_expr(
        &self,
        kind: ExprWrapperKind,
        inner: Option<ExprId>,
    ) -> (BodyResolution, Ty) {
        let Some(inner) = inner else {
            let ty = if matches!(kind, ExprWrapperKind::Return) {
                Ty::Never
            } else {
                Ty::Unknown
            };
            return (BodyResolution::Unknown, ty);
        };
        let inner_ty = self.pass.inference.root_resolved_expr_ty(inner);

        // Wrapper typing is intentionally shallow. Rust-glancer models `async fn` return types as
        // their declared output, and it does not yet model arbitrary `Future::Output` or `Try`
        // projections. Keep those omissions visible here, where wrapper expressions are resolved,
        // rather than presenting the small `Result`/`Option` heuristic as a general normalizer.
        let ty = match kind {
            ExprWrapperKind::Paren => inner_ty,
            ExprWrapperKind::Ref { mutability } => Ty::reference(mutability, inner_ty),
            ExprWrapperKind::Await => inner_ty,
            ExprWrapperKind::Try => {
                let mut outputs = ExpectedUnique::new();
                let item_query = self.pass.context().item_query();
                for nominal in inner_ty.as_adts() {
                    let Ok(Some(name)) = item_query.type_def_name(nominal.def) else {
                        continue;
                    };
                    if matches!(name, "Result" | "Option")
                        && let Some(output) =
                            nominal.args.iter().find_map(|arg| arg.as_ty().cloned())
                    {
                        outputs.push(output);
                    }
                }
                outputs.into_ty()
            }
            // `return expr` evaluates to `!`; the child expression remains separately lowered and
            // queryable, so callers can still ask about `expr` itself.
            ExprWrapperKind::Return => Ty::Never,
        };
        let resolution = if matches!(kind, ExprWrapperKind::Paren) {
            self.pass.expr_resolution(inner).clone()
        } else {
            BodyResolution::Unknown
        };

        (resolution, ty)
    }

    fn explicit_deref_ty(&self, inner: ExprId) -> Result<Ty, PackageStoreError> {
        let mut candidates = ExpectedUnique::new();
        let inner_ty = self.pass.inference.root_resolved_expr_ty(inner);
        for candidate in self
            .pass
            .context()
            .autoderef()
            .candidates(AutoderefMode::ExplicitDeref, &inner_ty)
        {
            candidates.push(candidate?.ty().clone());
        }

        Ok(candidates.into_ty())
    }
}
