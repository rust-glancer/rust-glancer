//! Expression resolution for the body-resolution fixed-point pass.
//!
//! This module owns expression-shaped traversal and the small type facts derived while walking
//! expressions. The parent resolver still drives pass ordering and binding propagation.

use rg_ir_model::{
    DefId, DefMapRef, ExprId, FunctionRef, ImplRef, ItemOwner, Path, ScopeId, SemanticItemRef,
    TypePathResolution,
    identity::DeclarationRef,
    items::{FieldKey, GenericArg as ItemGenericArg, GenericParams},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_ty::{
    AutoderefMode, CallArgInference, CallArgMapping, NominalTy, ReferencePeelingCandidates, Ty,
    TypeSubst, function_generic_shadow_subst, ty_for_binary, ty_for_literal, ty_for_unary,
};

use crate::{
    ExprData, ExprUnaryOp,
    ir::resolved::BodyResolution,
    ir::{ExprKind, ExprWrapperKind},
};

use super::{
    TypeRefUseSite,
    body::{BodyResolver, BodyValuePathResolver},
    normalize::TyNormalizer,
    push_unique,
};

pub(super) struct ExprResolver<'pass, 'query, 'body, D, I> {
    pass: &'pass mut BodyResolver<'query, 'body, D, I>,
}

impl<'pass, 'query, 'body, D, I> ExprResolver<'pass, 'query, 'body, D, I> {
    pub(super) fn new(pass: &'pass mut BodyResolver<'query, 'body, D, I>) -> Self {
        Self { pass }
    }
}

impl<'pass, 'query, 'body, D, I> ExprResolver<'pass, 'query, 'body, D, I>
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
                let (resolution, ty) = match path.as_def_map_path() {
                    Some(path) => self.resolve_path_expr(expr, &path)?,
                    None => (BodyResolution::Unknown, Ty::Unknown),
                };
                self.pass.body.set_expr_facts(expr, resolution, ty);
            }
            ExprKind::Call { callee, args } => {
                let ty = self.call_ty(callee, &args)?;
                self.pass.body.set_expr_ty(expr, ty);
            }
            ExprKind::Tuple { fields } => {
                let ty = self.tuple_expr_ty(&fields);
                self.pass.body.set_expr_ty(expr, ty);
            }
            ExprKind::Array { elements } => {
                let ty = self.array_expr_ty(&elements);
                self.pass.body.set_expr_ty(expr, ty);
            }
            ExprKind::RepeatArray {
                initializer,
                len_text,
                ..
            } => {
                let ty = self.repeat_array_expr_ty(initializer, len_text.as_deref());
                self.pass.body.set_expr_ty(expr, ty);
            }
            ExprKind::Index { base, .. } => {
                let ty = self.index_expr_ty(base);
                self.pass.body.set_expr_ty(expr, ty);
            }
            ExprKind::Cast { ty: Some(ty), .. } => {
                let ty = self
                    .pass
                    .type_path_resolver()
                    .type_ref(TypeRefUseSite::Scope(
                        self.pass.body.expr_unchecked(expr).scope,
                    ))
                    .resolve(&ty)?;
                self.pass.body.set_expr_ty(expr, ty);
            }
            ExprKind::Match { arms, .. } => {
                let mut arm_tys = Vec::new();
                for arm in arms {
                    if let Some(expr) = arm.expr {
                        push_unique(&mut arm_tys, self.pass.body.expr_ty_unchecked(expr).clone());
                    }
                }
                let ty = if arm_tys.len() == 1 {
                    arm_tys.pop().expect("one arm type should exist")
                } else {
                    Ty::Unknown
                };
                self.pass.body.set_expr_ty(expr, ty);
            }
            ExprKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                let ty = match else_branch {
                    Some(else_branch) => {
                        let mut branch_tys = Vec::new();
                        if let Some(then_branch) = then_branch {
                            push_unique(
                                &mut branch_tys,
                                self.pass.body.expr_ty_unchecked(then_branch).clone(),
                            );
                        }
                        push_unique(
                            &mut branch_tys,
                            self.pass.body.expr_ty_unchecked(else_branch).clone(),
                        );

                        if branch_tys.len() == 1 {
                            branch_tys.pop().expect("one branch type should exist")
                        } else {
                            Ty::Unknown
                        }
                    }
                    None => Ty::Unit,
                };
                self.pass.body.set_expr_ty(expr, ty);
            }
            ExprKind::Block { tail, .. } => {
                let ty = tail
                    .map(|tail| self.pass.body.expr_ty_unchecked(tail).clone())
                    .unwrap_or(Ty::Unit);
                self.pass.body.set_expr_ty(expr, ty);
            }
            ExprKind::Field { base, field, .. } => {
                let (resolution, ty) = self.resolve_field_expr(base, field.as_ref())?;
                self.pass.body.set_expr_facts(expr, resolution, ty);
            }
            ExprKind::Record { path, .. } => {
                let (resolution, ty) = match path.as_ref().and_then(|path| path.as_def_map_path()) {
                    Some(path) => self.resolve_record_expr_path(
                        self.pass.body.expr_unchecked(expr).scope,
                        &path,
                    )?,
                    None => (BodyResolution::Unknown, Ty::Unknown),
                };
                self.pass.body.set_expr_facts(expr, resolution, ty);
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
                self.pass.body.set_expr_facts(expr, resolution, ty);
            }
            ExprKind::Wrapper { kind, inner } => {
                let (resolution, ty) = self.resolve_wrapper_expr(kind, inner);
                self.pass.body.set_expr_facts(expr, resolution, ty);
            }
            ExprKind::Unary {
                op: Some(ExprUnaryOp::Deref),
                expr: Some(inner),
            } => {
                let ty = self.explicit_deref_ty(inner)?;
                self.pass.body.set_expr_ty(expr, ty);
            }
            ExprKind::Unary {
                op: Some(op),
                expr: Some(inner),
            } => {
                let ty = ty_for_unary(op, self.pass.body.expr_ty_unchecked(inner));
                self.pass.body.set_expr_ty(expr, ty);
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
                self.pass.body.set_expr_ty(expr, ty);
            }
            ExprKind::Literal { kind } => {
                self.pass.body.set_expr_ty(expr, ty_for_literal(kind));
            }
            ExprKind::While { .. } | ExprKind::For { .. } => {
                self.pass.body.set_expr_ty(expr, Ty::Unit);
            }
            ExprKind::Assign { .. } => {
                self.pass.body.set_expr_ty(expr, Ty::Unit);
            }
            ExprKind::Break { .. } | ExprKind::Continue { .. } => {
                self.pass.body.set_expr_ty(expr, Ty::Never);
            }
            ExprKind::Yeet { .. } | ExprKind::Become { .. } => {
                self.pass.body.set_expr_ty(expr, Ty::Never);
            }
            ExprKind::Let { .. }
            | ExprKind::Closure { .. }
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
        BodyValuePathResolver::new(self.pass.query_source(), Some(self.pass.semantic_index))
            .resolve_path_expr(scope, path, Some(visible_bindings))
    }

    fn tuple_expr_ty(&self, fields: &[ExprId]) -> Ty {
        Ty::tuple(
            fields
                .iter()
                .map(|field| self.pass.body.expr_ty_unchecked(*field).clone())
                .collect(),
        )
    }

    fn array_expr_ty(&self, elements: &[ExprId]) -> Ty {
        if elements.is_empty() {
            return Ty::Unknown;
        }

        let mut element_tys = Vec::new();
        for element in elements {
            let element_ty = self.pass.body.expr_ty_unchecked(*element).clone();
            if matches!(element_ty, Ty::Unknown) {
                return Ty::Unknown;
            }
            push_unique(&mut element_tys, element_ty);
        }

        if element_tys.len() == 1 {
            Ty::array(
                element_tys
                    .pop()
                    .expect("one array element type should exist"),
                Some(elements.len().to_string()),
            )
        } else {
            Ty::Unknown
        }
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

    pub(super) fn resolve_nonlocal_path_expr(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<(BodyResolution, Ty), PackageStoreError> {
        BodyValuePathResolver::new(self.pass.query_source(), Some(self.pass.semantic_index))
            .resolve_nonlocal_path_expr(scope, path)
    }

    fn resolve_record_expr_path(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<(BodyResolution, Ty), PackageStoreError> {
        match self
            .pass
            .type_path_resolver()
            .resolve_in_scope(scope, path)?
        {
            TypePathResolution::SelfType(types) => {
                return Ok((
                    BodyResolution::Unknown,
                    Ty::self_ty(types.into_iter().map(NominalTy::bare).collect()),
                ));
            }
            TypePathResolution::TypeDefs(types) => {
                let types = types
                    .into_iter()
                    .filter(|ty| ty.origin == DefMapRef::Body(self.pass.body_ref))
                    .collect::<Vec<_>>();
                if !types.is_empty() {
                    return Ok((
                        BodyResolution::Declarations(
                            types.iter().copied().map(DeclarationRef::from).collect(),
                        ),
                        Ty::nominal(types.into_iter().map(NominalTy::bare).collect()),
                    ));
                }
            }
            TypePathResolution::TypeAliases(_)
            | TypePathResolution::Traits(_)
            | TypePathResolution::Unknown => {}
        }

        self.resolve_nonlocal_path_expr(scope, path)
    }

    fn resolve_field_expr(
        &self,
        base: Option<ExprId>,
        field: Option<&FieldKey>,
    ) -> Result<(BodyResolution, Ty), PackageStoreError> {
        let (Some(base), Some(field)) = (base, field) else {
            return Ok((BodyResolution::Unknown, Ty::Unknown));
        };

        let item_query = self.pass.item_query();
        let mut current_depth = None;
        let mut fields = Vec::new();
        let mut field_tys = Vec::new();

        for candidate in self.pass.autoderef().candidates(
            AutoderefMode::FieldLookup,
            self.pass.body.expr_ty_unchecked(base),
        ) {
            let candidate = candidate?;
            // Autoderef yields candidates by depth. Resolve only after the whole matching depth is
            // collected, so same-depth alternatives produce ambiguity instead of order dependence.
            if current_depth.is_some_and(|depth| depth != candidate.depth())
                && (!fields.is_empty() || !field_tys.is_empty())
            {
                let ty = if field_tys.len() == 1 {
                    field_tys.pop().expect("one field type should exist")
                } else {
                    Ty::Unknown
                };
                let resolution = if fields.is_empty() {
                    BodyResolution::Unknown
                } else {
                    BodyResolution::Declarations(
                        fields.into_iter().map(DeclarationRef::from).collect(),
                    )
                };
                return Ok((resolution, ty));
            }
            current_depth = Some(candidate.depth());

            if let Some(field_ty) = Self::structural_field_ty(candidate.ty(), field) {
                push_unique(&mut field_tys, field_ty);
            }

            for nominal_ty in candidate.ty().as_nominals() {
                let Some(field_ref) = item_query.field_for_type(nominal_ty.def, field)? else {
                    continue;
                };
                push_unique(&mut fields, field_ref);

                let Some(field_data) = item_query.field_data(field_ref)? else {
                    continue;
                };
                let subst = self.semantic_type_subst(nominal_ty)?;
                let field_ty = self
                    .pass
                    .type_path_resolver()
                    .type_ref(TypeRefUseSite::Module(field_data.owner_module))
                    .with_subst(&subst)
                    .resolve(&field_data.field.ty)?;
                push_unique(&mut field_tys, field_ty);
            }
        }

        if !fields.is_empty() || !field_tys.is_empty() {
            let ty = if field_tys.len() == 1 {
                field_tys.pop().expect("one field type should exist")
            } else {
                Ty::Unknown
            };
            let resolution = if fields.is_empty() {
                BodyResolution::Unknown
            } else {
                BodyResolution::Declarations(fields.into_iter().map(DeclarationRef::from).collect())
            };
            return Ok((resolution, ty));
        }

        Ok((BodyResolution::Unknown, Ty::Unknown))
    }

    fn structural_field_ty(ty: &Ty, field: &FieldKey) -> Option<Ty> {
        match (ty, field) {
            (Ty::Tuple(fields), FieldKey::Tuple(index)) => fields.get(*index).cloned(),
            _ => None,
        }
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

        let receiver_ty = self.pass.body.expr_ty_unchecked(receiver);
        let item_query = self.pass.item_query();

        // Method lookup is intentionally shallow: nominal type plus lightweight impl-argument
        // matching gives useful candidates without modeling the full trait solver.
        let mut current_depth = None;
        let mut functions = Vec::new();
        let mut return_tys = Vec::new();

        for candidate in self
            .pass
            .autoderef()
            .candidates(AutoderefMode::MethodReceiver, receiver_ty)
        {
            let candidate = candidate?;
            // Autoderef yields candidates by depth. Resolve only after the whole matching depth is
            // collected, so same-depth alternatives produce ambiguity instead of order dependence.
            if current_depth.is_some_and(|depth| depth != candidate.depth())
                && !functions.is_empty()
            {
                let ty = if return_tys.len() == 1 {
                    return_tys.pop().expect("one return type should exist")
                } else {
                    Ty::Unknown
                };
                return Ok((
                    BodyResolution::Declarations(
                        functions.into_iter().map(DeclarationRef::from).collect(),
                    ),
                    ty,
                ));
            }
            current_depth = Some(candidate.depth());

            for nominal_ty in candidate.ty().as_nominals() {
                for function_ref in self
                    .pass
                    .receiver_functions()
                    .function_refs_for_receiver(nominal_ty, Some(method_name))?
                {
                    let Some(function_data) = item_query.function_data(function_ref)? else {
                        continue;
                    };
                    if function_data.name != method_name || !function_data.has_self_receiver() {
                        continue;
                    }

                    push_unique(&mut functions, function_ref);
                    push_unique(
                        &mut return_tys,
                        self.semantic_function_return_ty_with_call_args(
                            function_ref,
                            Some(nominal_ty),
                            explicit_args,
                            args,
                            CallArgMapping::MethodCall,
                            Some(call_scope),
                        )?,
                    );
                }
            }

            // Structural receivers such as `[T]` do not have a named type definition, so they
            // cannot use the nominal `TypeDefRef` impl index above. They still may have visible
            // inherent impls, for example `impl<T> [T]`, and those impls carry substitutions that
            // are needed to render returns like `&T` in the receiver context.
            for structural in self
                .pass
                .receiver_functions()
                .structural_function_candidates_for_receiver(candidate.ty(), Some(method_name))?
            {
                let function_ref = structural.function();
                let Some(function_data) = item_query.function_data(function_ref)? else {
                    continue;
                };
                if function_data.name != method_name || !function_data.has_self_receiver() {
                    continue;
                }

                push_unique(&mut functions, function_ref);
                push_unique(
                    &mut return_tys,
                    self.semantic_function_return_ty_with_subst_and_call_args(
                        function_ref,
                        Some(structural.receiver_ty().clone()),
                        structural.subst().clone(),
                        explicit_args,
                        args,
                        CallArgMapping::MethodCall,
                        Some(call_scope),
                    )?,
                );
            }
        }

        if !functions.is_empty() {
            let ty = if return_tys.len() == 1 {
                return_tys.pop().expect("one return type should exist")
            } else {
                Ty::Unknown
            };
            return Ok((
                BodyResolution::Declarations(
                    functions.into_iter().map(DeclarationRef::from).collect(),
                ),
                ty,
            ));
        }

        Ok((BodyResolution::Unknown, Ty::Unknown))
    }

    fn resolve_wrapper_expr(
        &self,
        kind: ExprWrapperKind,
        inner: Option<ExprId>,
    ) -> (BodyResolution, Ty) {
        let Some(inner) = inner else {
            return (BodyResolution::Unknown, Ty::Unknown);
        };
        let ty = TyNormalizer::new(self.pass.query_source())
            .ty_for_wrapper(kind, self.pass.body.expr_ty_unchecked(inner).clone());
        let resolution = if matches!(kind, ExprWrapperKind::Paren) {
            self.pass.body.expr_resolution(inner).clone()
        } else {
            BodyResolution::Unknown
        };

        (resolution, ty)
    }

    fn explicit_deref_ty(&self, inner: ExprId) -> Result<Ty, PackageStoreError> {
        let mut candidates = Vec::new();
        for candidate in self.pass.autoderef().candidates(
            AutoderefMode::ExplicitDeref,
            self.pass.body.expr_ty_unchecked(inner),
        ) {
            push_unique(&mut candidates, candidate?.ty().clone());
        }

        Ok(if candidates.len() == 1 {
            candidates
                .pop()
                .expect("one explicit deref candidate should exist")
        } else {
            Ty::Unknown
        })
    }

    fn impl_self_subst_for_function(
        &self,
        function_ref: FunctionRef,
        owner: ItemOwner,
        receiver_ty: &NominalTy,
    ) -> Result<TypeSubst, PackageStoreError> {
        let ItemOwner::Impl(impl_id) = owner else {
            return Ok(TypeSubst::new());
        };
        let item_query = self.pass.item_query();
        let Some(impl_data) = item_query.impl_data(ImplRef {
            origin: function_ref.origin,
            id: impl_id,
        })?
        else {
            return Ok(TypeSubst::new());
        };

        Ok(self
            .pass
            .impl_matcher()
            .impl_self_subst_for_impl(impl_data, receiver_ty))
    }

    fn semantic_type_subst(&self, ty: &NominalTy) -> Result<TypeSubst, PackageStoreError> {
        Ok(self
            .pass
            .item_query()
            .generic_params_for_type_def(ty.def)?
            .map(|generics| TypeSubst::from_generics(generics, &ty.args))
            .unwrap_or_else(TypeSubst::new))
    }

    fn semantic_function_return_ty_with_call_args(
        &self,
        function_ref: FunctionRef,
        receiver_ty: Option<&NominalTy>,
        explicit_args: &[ItemGenericArg],
        args: &[ExprId],
        arg_mapping: CallArgMapping,
        call_scope: Option<ScopeId>,
    ) -> Result<Ty, PackageStoreError> {
        let Some(function_data) = self.pass.item_query().function_data(function_ref)? else {
            return Ok(Ty::Unknown);
        };
        let subst = receiver_ty
            .map(|ty| {
                // Receiver type args and impl self args both contribute substitutions. For
                // `impl<U> Wrapper<U>`, this maps `U` to the known receiver argument.
                let mut subst = self.semantic_type_subst(ty)?;
                subst.extend(self.impl_self_subst_for_function(
                    function_ref,
                    function_data.owner,
                    ty,
                )?);
                Ok(subst)
            })
            .transpose()?
            .unwrap_or_default();
        self.semantic_function_return_ty_with_subst_and_call_args(
            function_ref,
            receiver_ty.cloned().map(|ty| Ty::nominal(vec![ty])),
            subst,
            explicit_args,
            args,
            arg_mapping,
            call_scope,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn semantic_function_return_ty_with_subst_and_call_args(
        &self,
        function_ref: FunctionRef,
        self_ty: Option<Ty>,
        mut subst: TypeSubst,
        explicit_args: &[ItemGenericArg],
        args: &[ExprId],
        arg_mapping: CallArgMapping,
        call_scope: Option<ScopeId>,
    ) -> Result<Ty, PackageStoreError> {
        let item_query = self.pass.item_query();
        let Some(function_data) = item_query.function_data(function_ref)? else {
            return Ok(Ty::Unknown);
        };
        subst.extend(function_generic_shadow_subst(
            function_data.signature.generics(),
        ));
        if let Some(call_scope) = call_scope {
            subst.extend(self.explicit_function_subst(
                function_data.signature.generics(),
                explicit_args,
                call_scope,
            )?);
        }
        let arg_tys = args
            .iter()
            .map(|arg| self.pass.body.expr_ty_unchecked(*arg).clone())
            .collect::<Vec<_>>();
        let inferred_subst = CallArgInference::new(
            function_data.signature.generics(),
            function_data.signature.params(),
            &arg_tys,
            arg_mapping,
            &subst,
        )
        .infer();
        subst.extend(inferred_subst);
        self.semantic_function_return_ty_with_resolved_subst(function_ref, self_ty, subst)
    }

    fn semantic_function_return_ty_with_resolved_subst(
        &self,
        function_ref: FunctionRef,
        self_ty: Option<Ty>,
        subst: TypeSubst,
    ) -> Result<Ty, PackageStoreError> {
        let item_query = self.pass.item_query();
        let Some(function_data) = item_query.function_data(function_ref)? else {
            return Ok(Ty::Unknown);
        };
        let Some(ret_ty) = function_data.signature.ret_ty() else {
            return Ok(Ty::Unit);
        };

        if ret_ty.is_self_type() {
            return Ok(match self_ty {
                Some(self_ty) => self_ty,
                None => Ty::self_ty(
                    self.pass
                        .type_path_resolver()
                        .self_nominal_tys_for_function(function_ref)?,
                ),
            });
        }

        self.pass
            .type_path_resolver()
            .type_ref(TypeRefUseSite::Function(function_ref))
            .with_subst(&subst)
            .resolve(ret_ty)
    }

    fn call_ty(&self, callee: Option<ExprId>, args: &[ExprId]) -> Result<Ty, PackageStoreError> {
        let Some(callee) = callee else {
            return Ok(Ty::Unknown);
        };
        let callee_data = self.pass.body.expr_unchecked(callee);
        let callee_ty = self.pass.body.expr_ty_unchecked(callee);

        if matches!(callee_ty, Ty::Nominal(_) | Ty::SelfTy(_)) {
            return Ok(callee_ty.clone());
        }

        // Ordinary calls use declared return types plus a deliberately-small substitution model:
        // explicit turbofish args and direct argument-to-parameter type inference.
        let mut return_tys = Vec::new();
        match self.pass.body.expr_resolution(callee) {
            BodyResolution::Declarations(declarations) => {
                for declaration in declarations {
                    self.push_return_ty_for_declaration(
                        *declaration,
                        &mut return_tys,
                        self.explicit_callee_generic_args(callee_data),
                        callee_data.scope,
                        args,
                    )?;
                }
            }
            BodyResolution::Binding(_) | BodyResolution::Unknown => {}
        }

        if return_tys.len() == 1 {
            Ok(return_tys.pop().expect("one return type should exist"))
        } else {
            Ok(Ty::Unknown)
        }
    }

    fn push_return_ty_for_declaration(
        &self,
        declaration: DeclarationRef,
        return_tys: &mut Vec<Ty>,
        explicit_args: &[ItemGenericArg],
        call_scope: ScopeId,
        args: &[ExprId],
    ) -> Result<(), PackageStoreError> {
        match declaration {
            DeclarationRef::LocalDef(local_def) => {
                let Some(function_ref) = self.function_ref_for_def(DefId::Local(local_def))? else {
                    return Ok(());
                };
                push_unique(
                    return_tys,
                    self.semantic_function_return_ty_with_call_args(
                        function_ref,
                        None,
                        explicit_args,
                        args,
                        CallArgMapping::FunctionCall,
                        Some(call_scope),
                    )?,
                );
            }
            DeclarationRef::Item(SemanticItemRef::Function(function_ref)) => {
                push_unique(
                    return_tys,
                    self.semantic_function_return_ty_with_call_args(
                        function_ref,
                        None,
                        explicit_args,
                        args,
                        CallArgMapping::FunctionCall,
                        Some(call_scope),
                    )?,
                );
            }
            DeclarationRef::Module(_)
            | DeclarationRef::Item(
                SemanticItemRef::TypeDef(_)
                | SemanticItemRef::Trait(_)
                | SemanticItemRef::Impl(_)
                | SemanticItemRef::TypeAlias(_)
                | SemanticItemRef::Const(_)
                | SemanticItemRef::Static(_),
            )
            | DeclarationRef::Field(_)
            | DeclarationRef::EnumVariant(_)
            | DeclarationRef::BodyBinding(_) => {}
        }

        Ok(())
    }

    fn explicit_callee_generic_args<'expr>(
        &self,
        callee_data: &'expr ExprData,
    ) -> &'expr [ItemGenericArg] {
        // A normal call expression has a callee expression, so `make::<T>()` and
        // `Type::build::<T>()` carry call generics on the final callee path segment. Method calls
        // are a different ExprKind and store their method-name generics directly.
        match &callee_data.kind {
            ExprKind::Path { path } => path.last_segment_angle_args().unwrap_or(&[]),
            _ => &[],
        }
    }

    fn explicit_function_subst(
        &self,
        generics: Option<&GenericParams>,
        explicit_args: &[ItemGenericArg],
        scope: ScopeId,
    ) -> Result<TypeSubst, PackageStoreError> {
        let Some(generics) = generics else {
            return Ok(TypeSubst::new());
        };
        if explicit_args.is_empty() {
            return Ok(TypeSubst::new());
        }

        // Function turbofish arguments are supplied at the call site, so names inside them must
        // resolve from the body scope where the call was written.
        let type_resolver = self.pass.type_path_resolver();
        let arg_resolver = type_resolver.type_ref(TypeRefUseSite::Scope(scope));
        let generic_args = explicit_args
            .iter()
            .map(|arg| arg_resolver.generic_arg(arg))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TypeSubst::from_generics(generics, &generic_args))
    }

    fn function_ref_for_def(&self, def: DefId) -> Result<Option<FunctionRef>, PackageStoreError> {
        let DefId::Local(local_def) = def else {
            return Ok(None);
        };
        Ok(
            match self
                .pass
                .item_query()
                .semantic_item_for_local_def(local_def)?
            {
                Some(SemanticItemRef::Function(function)) => Some(function),
                Some(_) | None => None,
            },
        )
    }
}
