//! Expression resolution for the body-resolution fixed-point pass.
//!
//! This module owns expression-shaped traversal and the small type facts derived while walking
//! expressions. The parent pass still drives ordering and binding propagation.

use rg_ir_model::{
    BodyPath, DefMapRef, ExprId, Path, ScopeId, TypeDefRef, TypePathResolution,
    identity::DeclarationRef,
    items::{FieldKey, GenericArg as ItemGenericArg},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_std::ExpectedUnique;
use rg_ty::{
    AutoderefMode, ExpectedTyExt, GenericArg, NominalTy, PrimitiveTy, ReferencePeelingCandidates,
    Ty, ty_for_binary, ty_for_literal, ty_for_unary,
};

use crate::{
    ExprUnaryOp,
    ir::resolved::BodyResolution,
    ir::{ExprKind, ExprWrapperKind, LiteralKind},
};

use crate::resolution::{CallSite, MethodCallSite, TypeRefUseSite, support::TyNormalizer};

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
        let old_resolution = self.pass.body.expr_resolution(expr).clone();
        let old_ty = self.pass.body.expr_ty_unchecked(expr).clone();
        let expr_data = self.pass.body.expr_unchecked(expr);
        let kind = expr_data.kind.clone();

        match kind {
            ExprKind::Path { path } => {
                let (resolution, ty) = self.resolve_body_path_expr(expr, &path)?;
                self.pass.set_expr_facts(expr, resolution, ty);
            }
            ExprKind::Call { callee, args } => {
                let ty = self.pass.context().calls().call_expr_ty(callee, &args)?;
                self.pass.set_expr_ty(expr, ty);
            }
            ExprKind::BuiltinMacro { kind } => {
                let ty = BuiltinMacroExprTypeMapper::new(self.pass.context()).ty_for(expr, kind)?;
                self.pass.set_expr_ty(expr, ty);
            }
            ExprKind::Tuple { fields } => {
                self.pass.set_expr_tuple_from_fields(expr, &fields);
            }
            ExprKind::Array { elements } => {
                let ty = self.array_expr_ty(&elements);
                self.pass.set_expr_array_from_elements(expr, &elements, ty);
            }
            ExprKind::RepeatArray {
                initializer,
                len_text,
                ..
            } => {
                let ty = self.repeat_array_expr_ty(initializer, len_text.as_deref());
                self.pass.set_expr_repeat_array_from_initializer(
                    expr,
                    initializer,
                    len_text.as_deref(),
                    ty,
                );
            }
            ExprKind::Index { base, .. } => {
                let ty = self.index_expr_ty(base);
                self.pass.set_expr_ty(expr, ty);
            }
            ExprKind::Cast { ty: Some(ty), .. } => {
                let ty = self
                    .pass
                    .context()
                    .type_refs(TypeRefUseSite::Scope(
                        self.pass.body.expr_unchecked(expr).scope,
                    ))
                    .resolve(&ty)?;
                self.pass.set_expr_ty(expr, ty);
            }
            ExprKind::Match { arms, .. } => {
                let mut arm_tys = ExpectedUnique::new();
                for arm in arms {
                    if let Some(expr) = arm.expr {
                        arm_tys.push(self.pass.body.expr_ty_unchecked(expr).clone());
                    }
                }
                let ty = arm_tys.into_ty();
                self.pass.set_expr_ty(expr, ty);
            }
            ExprKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                let ty = match else_branch {
                    Some(else_branch) => {
                        let mut branch_tys = ExpectedUnique::new();
                        if let Some(then_branch) = then_branch {
                            branch_tys.push(self.pass.body.expr_ty_unchecked(then_branch).clone());
                        }
                        branch_tys.push(self.pass.body.expr_ty_unchecked(else_branch).clone());

                        branch_tys.into_ty()
                    }
                    None => Ty::Unit,
                };
                self.pass.set_expr_ty(expr, ty);
            }
            ExprKind::Block { tail, .. } => {
                let ty = tail
                    .map(|tail| self.pass.body.expr_ty_unchecked(tail).clone())
                    .unwrap_or(Ty::Unit);
                self.pass.set_expr_ty(expr, ty);
            }
            ExprKind::Field { base, field, .. } => {
                let (resolution, ty) = self.resolve_field_expr(base, field.as_ref())?;
                self.pass.set_expr_facts(expr, resolution, ty);
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
            ExprKind::MethodCall {
                receiver,
                method_name,
                generic_args,
                args,
                ..
            } => {
                let (resolution, ty) = self.resolve_method_call_expr(
                    receiver,
                    &method_name,
                    &generic_args,
                    &args,
                    self.pass.body.expr_unchecked(expr).scope,
                )?;
                self.pass.set_expr_facts(expr, resolution, ty);
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
                let ty = ty_for_unary(op, self.pass.body.expr_ty_unchecked(inner));
                self.pass.set_expr_ty(expr, ty);
            }
            ExprKind::Binary {
                lhs: Some(lhs),
                rhs: Some(rhs),
                op: Some(op),
            } => {
                let ty = ty_for_binary(
                    op,
                    self.pass.body.expr_ty_unchecked(lhs),
                    self.pass.body.expr_ty_unchecked(rhs),
                );
                self.pass.set_expr_ty(expr, ty);
            }
            ExprKind::Literal { kind } => match kind {
                LiteralKind::Int { primitive_ty: None } => self.pass.set_expr_integer_var(expr),
                LiteralKind::Float { primitive_ty: None } => self.pass.set_expr_float_var(expr),
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

        Ok(self.pass.body.expr_resolution(expr) != &old_resolution
            || self.pass.body.expr_ty_unchecked(expr) != &old_ty)
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

    fn array_expr_ty(&self, elements: &[ExprId]) -> Ty {
        if elements.is_empty() {
            return Ty::Unknown;
        }

        let mut element_tys = ExpectedUnique::new();
        for element in elements {
            let element_ty = self.pass.body.expr_ty_unchecked(*element).clone();
            if matches!(element_ty, Ty::Unknown) {
                return Ty::Unknown;
            }
            element_tys.push(element_ty);
        }

        element_tys
            .map(|element_ty| Ty::array(element_ty, Some(elements.len().to_string())))
            .into_ty()
    }

    fn repeat_array_expr_ty(&self, initializer: Option<ExprId>, len_text: Option<&str>) -> Ty {
        let Some(initializer) = initializer else {
            return Ty::Unknown;
        };

        Ty::array(
            self.pass.body.expr_ty_unchecked(initializer).clone(),
            len_text.map(str::to_owned),
        )
    }

    fn index_expr_ty(&self, base: Option<ExprId>) -> Ty {
        let Some(base) = base else {
            return Ty::Unknown;
        };

        // Indexing is reference-transparent for the structural array/slice cases we model here:
        // `&[T]` and `&[T; N]` should behave like their inner container. Keep this deliberately
        // narrower than method lookup: no trait deref, no `Index` trait, and no container coercions.
        for candidate in ReferencePeelingCandidates::new(self.pass.body.expr_ty_unchecked(base)) {
            match candidate.ty() {
                Ty::Array { inner, .. } | Ty::Slice(inner) => return inner.as_ref().clone(),
                _ => {}
            }
        }

        Ty::Unknown
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
                    Ty::self_ty(self.record_nominal_ty(scope, path, type_def)?),
                ));
            }
            TypePathResolution::TypeDef(type_def) => {
                let declaration = self.record_declaration_for_type_def(type_def)?;
                return Ok((
                    BodyResolution::Declarations([declaration].into_iter().collect()),
                    Ty::nominal(self.record_nominal_ty(scope, path, type_def)?),
                ));
            }
            TypePathResolution::TypeAlias(_)
            | TypePathResolution::Trait(_)
            | TypePathResolution::Unknown => {}
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
        if type_def.origin == DefMapRef::Body(self.pass.providers.body_ref()) {
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
    ) -> Result<NominalTy, PackageStoreError> {
        Ok(NominalTy {
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
    ) -> Result<Vec<GenericArg>, PackageStoreError> {
        if let Some(args) = path.last_segment_angle_args() {
            return self
                .pass
                .context()
                .type_refs(TypeRefUseSite::Scope(scope))
                .resolve_generic_args(args);
        }

        let Some(generics) = self
            .pass
            .context()
            .item_query()
            .generic_params_for_type_def(type_def)?
        else {
            return Ok(Vec::new());
        };

        // TODO: Omitted record constructor args should preserve non-type generic arity too.
        // We need a deliberate placeholder shape for lifetimes and consts before adding that.
        Ok(generics
            .types
            .iter()
            .map(|_| GenericArg::Type(Box::new(Ty::Unknown)))
            .collect())
    }

    fn resolve_field_expr(
        &self,
        base: Option<ExprId>,
        field: Option<&FieldKey>,
    ) -> Result<(BodyResolution, Ty), PackageStoreError> {
        let (Some(base), Some(field)) = (base, field) else {
            return Ok((BodyResolution::Unknown, Ty::Unknown));
        };

        let targets = self.pass.context().fields().resolve(base, field)?;
        if targets.is_empty() {
            return Ok((BodyResolution::Unknown, Ty::Unknown));
        }

        Ok((targets.resolution(), targets.ty()))
    }

    fn resolve_method_call_expr(
        &self,
        receiver: Option<ExprId>,
        method_name: &str,
        explicit_args: &[ItemGenericArg],
        args: &[ExprId],
        call_scope: ScopeId,
    ) -> Result<(BodyResolution, Ty), PackageStoreError> {
        let Some(receiver) = receiver else {
            return Ok((BodyResolution::Unknown, Ty::Unknown));
        };

        let calls = self.pass.context().calls();
        let targets = calls.targets(CallSite::Method(MethodCallSite {
            receiver,
            name: method_name,
            explicit_args,
            scope: call_scope,
        }))?;
        if targets.is_empty() {
            return Ok((BodyResolution::Unknown, Ty::Unknown));
        }

        Ok((targets.resolution(), targets.return_ty(&calls, args)?))
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
        let ty = TyNormalizer::new(self.pass.context())
            .ty_for_wrapper(kind, self.pass.body.expr_ty_unchecked(inner).clone());
        let resolution = if matches!(kind, ExprWrapperKind::Paren) {
            self.pass.body.expr_resolution(inner).clone()
        } else {
            BodyResolution::Unknown
        };

        (resolution, ty)
    }

    fn explicit_deref_ty(&self, inner: ExprId) -> Result<Ty, PackageStoreError> {
        let mut candidates = ExpectedUnique::new();
        for candidate in self.pass.context().autoderef().candidates(
            AutoderefMode::ExplicitDeref,
            self.pass.body.expr_ty_unchecked(inner),
        ) {
            candidates.push(candidate?.ty().clone());
        }

        Ok(candidates.into_ty())
    }
}
