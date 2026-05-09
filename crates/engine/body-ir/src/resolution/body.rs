//! Main body-resolution pass.
//!
//! This module walks lowered bodies and fills resolution/type slots on bindings and expressions.
//! Specialized helpers live in sibling modules so this file can read like the pass itself.

use rg_def_map::{DefId, DefMapReadTxn, Path, PathSegment};
use rg_item_tree::{FieldKey, TypeRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{FunctionRef, SemanticIrReadTxn, TypeDefId, TypePathContext};

use crate::{
    body::BodyData,
    expr::{ExprKind, ExprWrapperKind},
    ids::{
        BindingId, BodyFieldRef, BodyFunctionRef, BodyImplId, BodyItemRef, BodyRef, ExprId, ScopeId,
    },
    item::BodyFunctionOwner,
    resolved::{BodyResolution, BodyTypePathResolution, ResolvedFieldRef, ResolvedFunctionRef},
    stmt::BindingKind,
    ty::{BodyLocalNominalTy, BodyNominalTy, BodyTy},
};

use super::{
    SemanticResolutionIndex,
    method::{
        local_function_applies_to_receiver, local_impl_self_subst,
        semantic_function_applies_to_receiver, semantic_impl_self_subst,
        semantic_trait_function_candidates_for_receiver,
    },
    normalize::BodyTyNormalizer,
    pat::PatternTypePropagator,
    push_unique,
    ty::{TypeSubst, subst_from_generics, type_ref_is_self},
    type_path::BodyTypePathResolver,
};

pub(crate) struct BodyResolver<'query, 'db, 'body> {
    def_map: &'query DefMapReadTxn<'db>,
    semantic_ir: &'query SemanticIrReadTxn<'db>,
    semantic_index: &'query SemanticResolutionIndex,
    body_ref: BodyRef,
    body: &'body mut BodyData,
}

impl<'query, 'db, 'body> BodyResolver<'query, 'db, 'body> {
    pub(crate) fn new(
        def_map: &'query DefMapReadTxn<'db>,
        semantic_ir: &'query SemanticIrReadTxn<'db>,
        semantic_index: &'query SemanticResolutionIndex,
        body_ref: BodyRef,
        body: &'body mut BodyData,
    ) -> Self {
        Self {
            def_map,
            semantic_ir,
            semantic_index,
            body_ref,
            body,
        }
    }

    fn type_path_resolver(&self) -> BodyTypePathResolver<'_, 'db, '_> {
        BodyTypePathResolver::new(self.def_map, self.semantic_ir, self.body_ref, self.body)
    }

    pub(crate) fn resolve(&mut self) -> Result<(), PackageStoreError> {
        self.resolve_bindings()?;
        self.resolve_local_impls()?;

        // Pattern propagation can unlock later expression types, and those expressions can then
        // unlock more patterns. Every successful pass should discover at least one new binding or
        // expression fact, so a body-sized cap is enough to avoid a hidden magic constant.
        let max_passes = self.body.exprs.len() + self.body.bindings.len() + 1;
        for _ in 0..max_passes {
            let mut changed = false;
            for expr_idx in 0..self.body.exprs.len() {
                changed |= self.resolve_expr(ExprId(expr_idx))?;
            }
            changed |= PatternTypePropagator::new(
                self.def_map,
                self.semantic_ir,
                self.body_ref,
                self.body,
            )
            .propagate()?;

            if !changed {
                break;
            }
        }

        Ok(())
    }

    fn resolve_bindings(&mut self) -> Result<(), PackageStoreError> {
        for binding_idx in 0..self.body.bindings.len() {
            let binding = BindingId(binding_idx);
            let ty = self.binding_ty(binding)?;
            self.body.bindings[binding].ty = ty;
        }
        Ok(())
    }

    fn binding_ty(&self, binding: BindingId) -> Result<BodyTy, PackageStoreError> {
        let binding_data = &self.body.bindings[binding];
        if let Some(annotation) = &binding_data.annotation {
            return self
                .type_path_resolver()
                .ty_from_type_ref_in_scope(annotation, binding_data.scope);
        }

        if matches!(binding_data.kind, BindingKind::SelfParam)
            && binding_data.name.as_deref() == Some("self")
        {
            let self_tys = self
                .type_path_resolver()
                .self_tys_for_function(self.body.owner)?;
            if !self_tys.is_empty() {
                return Ok(BodyTy::SelfTy(
                    self_tys.into_iter().map(BodyNominalTy::bare).collect(),
                ));
            }
        }

        Ok(BodyTy::Unknown)
    }

    fn resolve_local_impls(&mut self) -> Result<(), PackageStoreError> {
        // Local impls are lowered before their `Self` type is known. Resolve that link once so
        // method lookup can match directly by body-local item identity.
        for impl_idx in 0..self.body.local_impls.len() {
            let impl_id = BodyImplId(impl_idx);
            let self_item = {
                let impl_data = &self.body.local_impls[impl_id];
                self.type_path_resolver()
                    .local_item_from_type_ref_in_scope(&impl_data.self_ty, impl_data.scope)?
            };

            if let Some(impl_data) = self.body.local_impl_mut(impl_id) {
                impl_data.self_item = self_item;
            }
        }
        Ok(())
    }

    fn resolve_expr(&mut self, expr: ExprId) -> Result<bool, PackageStoreError> {
        let old_resolution = self.body.exprs[expr].resolution.clone();
        let old_ty = self.body.exprs[expr].ty.clone();
        let kind = self.body.exprs[expr].kind.clone();

        match kind {
            ExprKind::Path { path } => {
                let (resolution, ty) = self.resolve_path_expr(expr, &path.path)?;
                let data = &mut self.body.exprs[expr];
                data.resolution = resolution;
                data.ty = ty;
            }
            ExprKind::Call { callee, .. } => {
                self.body.exprs[expr].ty = self.call_ty(callee)?;
            }
            ExprKind::Match { arms, .. } => {
                let mut arm_tys = Vec::new();
                for arm in arms {
                    if let Some(expr) = arm.expr {
                        push_unique(&mut arm_tys, self.body.exprs[expr].ty.clone());
                    }
                }
                self.body.exprs[expr].ty = if arm_tys.len() == 1 {
                    arm_tys.pop().expect("one arm type should exist")
                } else {
                    BodyTy::Unknown
                };
            }
            ExprKind::Block { tail, .. } => {
                self.body.exprs[expr].ty = tail
                    .map(|tail| self.body.exprs[tail].ty.clone())
                    .unwrap_or(BodyTy::Unit);
            }
            ExprKind::Field { base, field, .. } => {
                let (resolution, ty) = self.resolve_field_expr(base, field.as_ref())?;
                let data = &mut self.body.exprs[expr];
                data.resolution = resolution;
                data.ty = ty;
            }
            ExprKind::MethodCall {
                receiver,
                method_name,
                ..
            } => {
                let (resolution, ty) = self.resolve_method_call_expr(receiver, &method_name)?;
                let data = &mut self.body.exprs[expr];
                data.resolution = resolution;
                data.ty = ty;
            }
            ExprKind::Wrapper { kind, inner } => {
                let (resolution, ty) = self.resolve_wrapper_expr(kind, inner);
                let data = &mut self.body.exprs[expr];
                data.resolution = resolution;
                data.ty = ty;
            }
            ExprKind::Literal { .. } | ExprKind::Unknown { .. } => {}
        }

        Ok(
            self.body.exprs[expr].resolution != old_resolution
                || self.body.exprs[expr].ty != old_ty,
        )
    }

    fn resolve_path_expr(
        &self,
        expr: ExprId,
        path: &Path,
    ) -> Result<(BodyResolution, BodyTy), PackageStoreError> {
        let scope = self.body.exprs[expr].scope;
        if let Some(name) = path.single_name() {
            if let Some(binding) = self.resolve_local_name(scope, name, expr) {
                let ty = self.body.bindings[binding].ty.clone();
                return Ok((BodyResolution::Local(binding), ty));
            }
        }

        self.resolve_nonlocal_path_expr(scope, path)
    }

    pub(super) fn resolve_nonlocal_path_expr(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<(BodyResolution, BodyTy), PackageStoreError> {
        BodyValuePathResolver::new(
            self.def_map,
            self.semantic_ir,
            Some(self.semantic_index),
            self.body_ref,
            self.body,
        )
        .resolve_nonlocal_path_expr(scope, path)
    }

    fn resolve_field_expr(
        &self,
        base: Option<ExprId>,
        field: Option<&FieldKey>,
    ) -> Result<(BodyResolution, BodyTy), PackageStoreError> {
        let (Some(base), Some(field)) = (base, field) else {
            return Ok((BodyResolution::Unknown, BodyTy::Unknown));
        };

        let mut fields = Vec::new();
        let mut field_tys = Vec::new();

        // Local and semantic fields use the same substitution idea, but local items need their
        // declaration scope so field types can mention body-local names.
        for local_ty in self.body.exprs[base].ty.local_nominals() {
            let Some(field_ref) = self.local_field_for_type(local_ty.item, field) else {
                continue;
            };
            push_unique(&mut fields, ResolvedFieldRef::BodyLocal(field_ref));

            let Some(item) = self.body.local_item(field_ref.item.item) else {
                continue;
            };
            let Some(field_data) = item.field(field_ref.index) else {
                continue;
            };
            let subst = self.local_type_subst(local_ty);
            let field_ty = self
                .type_path_resolver()
                .ty_from_type_ref_in_scope_with_subst(&field_data.ty, item.scope, &subst)?;
            push_unique(&mut field_tys, field_ty);
        }

        for nominal_ty in self.body.exprs[base].ty.nominal_tys() {
            let Some(field_ref) = self.semantic_ir.field_for_type(nominal_ty.def, field)? else {
                continue;
            };
            push_unique(&mut fields, ResolvedFieldRef::Semantic(field_ref));

            let Some(field_data) = self.semantic_ir.field_data(field_ref)? else {
                continue;
            };
            let subst = self.semantic_type_subst(nominal_ty)?;
            let field_ty = self
                .type_path_resolver()
                .ty_from_type_ref_in_context_with_subst(
                    &field_data.field.ty,
                    TypePathContext::module(field_data.owner_module),
                    &subst,
                )?;
            push_unique(&mut field_tys, field_ty);
        }

        let resolution = if !fields.is_empty() {
            BodyResolution::Field(fields)
        } else {
            BodyResolution::Unknown
        };

        let ty = if field_tys.len() == 1 {
            field_tys.pop().expect("one field type should exist")
        } else {
            BodyTy::Unknown
        };

        Ok((resolution, ty))
    }

    fn resolve_method_call_expr(
        &self,
        receiver: Option<ExprId>,
        method_name: &str,
    ) -> Result<(BodyResolution, BodyTy), PackageStoreError> {
        let Some(receiver) = receiver else {
            return Ok((BodyResolution::Unknown, BodyTy::Unknown));
        };

        let mut functions = Vec::new();
        let mut return_tys = Vec::new();
        let receiver_ty = &self.body.exprs[receiver].ty;

        // Method lookup is intentionally shallow: exact local item identity for body-local impls,
        // and nominal type plus lightweight impl-argument matching for semantic impls.
        for local_ty in receiver_ty.local_nominals() {
            for function_ref in self.local_functions_for_type(local_ty)? {
                let Some(function_data) = self.body.local_function(function_ref.function) else {
                    continue;
                };
                if function_data.name != method_name || !function_data.has_self_receiver() {
                    continue;
                }

                push_unique(&mut functions, ResolvedFunctionRef::BodyLocal(function_ref));
                push_unique(
                    &mut return_tys,
                    self.local_function_return_ty(function_ref, Some(local_ty))?,
                );
            }
        }

        for nominal_ty in receiver_ty.nominal_tys() {
            for function_ref in self.semantic_functions_for_type(nominal_ty, method_name)? {
                let Some(function_data) = self.semantic_ir.function_data(function_ref)? else {
                    continue;
                };
                if function_data.name != method_name || !function_data.has_self_receiver() {
                    continue;
                }

                push_unique(&mut functions, ResolvedFunctionRef::Semantic(function_ref));
                push_unique(
                    &mut return_tys,
                    self.semantic_function_return_ty(function_ref, Some(nominal_ty))?,
                );
            }
        }

        let resolution = if functions.is_empty() {
            BodyResolution::Unknown
        } else {
            BodyResolution::Method(functions)
        };
        let ty = if return_tys.len() == 1 {
            return_tys.pop().expect("one return type should exist")
        } else {
            BodyTy::Unknown
        };

        Ok((resolution, ty))
    }

    fn resolve_wrapper_expr(
        &self,
        kind: ExprWrapperKind,
        inner: Option<ExprId>,
    ) -> (BodyResolution, BodyTy) {
        let Some(inner) = inner else {
            return (BodyResolution::Unknown, BodyTy::Unknown);
        };
        let inner_data = &self.body.exprs[inner];
        let ty = BodyTyNormalizer::new(self.semantic_ir, self.body)
            .ty_for_wrapper(kind, inner_data.ty.clone());
        let resolution = if matches!(kind, ExprWrapperKind::Paren) {
            inner_data.resolution.clone()
        } else {
            BodyResolution::Unknown
        };

        (resolution, ty)
    }

    fn local_field_for_type(&self, item_ref: BodyItemRef, key: &FieldKey) -> Option<BodyFieldRef> {
        let body = if item_ref.body == self.body_ref {
            &*self.body
        } else {
            return None;
        };
        let item = body.local_item(item_ref.item)?;
        let index = item.field_index(key)?;

        Some(BodyFieldRef {
            item: item_ref,
            index,
        })
    }

    fn local_functions_for_type(
        &self,
        ty: &BodyLocalNominalTy,
    ) -> Result<Vec<BodyFunctionRef>, PackageStoreError> {
        if ty.item.body != self.body_ref {
            return Ok(Vec::new());
        }

        let functions = self
            .body
            .inherent_functions_for_local_type(self.body_ref, ty.item);
        let mut retained = Vec::new();
        for function in functions {
            if local_function_applies_to_receiver(
                self.def_map,
                self.semantic_ir,
                self.body_ref,
                self.body,
                function,
                ty,
            )? {
                retained.push(function);
            }
        }
        Ok(retained)
    }

    fn semantic_functions_for_type(
        &self,
        ty: &BodyNominalTy,
        method_name: &str,
    ) -> Result<Vec<FunctionRef>, PackageStoreError> {
        let mut functions = Vec::new();
        for function in self
            .semantic_index
            .inherent_functions_for_type_and_name(ty.def, method_name)
            .to_vec()
        {
            if semantic_function_applies_to_receiver(self.def_map, self.semantic_ir, function, ty)?
            {
                functions.push(function);
            }
        }

        for (function, _) in semantic_trait_function_candidates_for_receiver(
            Some(self.semantic_index),
            self.def_map,
            self.semantic_ir,
            ty,
            Some(method_name),
        )? {
            push_unique(&mut functions, function);
        }
        Ok(functions)
    }

    fn local_type_subst(&self, ty: &BodyLocalNominalTy) -> TypeSubst {
        let Some(item) = self.body.local_item(ty.item.item) else {
            return TypeSubst::new();
        };

        subst_from_generics(&item.generics, &ty.args)
    }

    fn semantic_type_subst(&self, ty: &BodyNominalTy) -> Result<TypeSubst, PackageStoreError> {
        Ok(self
            .semantic_ir
            .generic_params_for_type_def(ty.def)?
            .map(|generics| subst_from_generics(generics, &ty.args))
            .unwrap_or_else(TypeSubst::new))
    }

    fn local_function_return_ty(
        &self,
        function_ref: BodyFunctionRef,
        receiver_ty: Option<&BodyLocalNominalTy>,
    ) -> Result<BodyTy, PackageStoreError> {
        let Some(function_data) = self.body.local_function(function_ref.function) else {
            return Ok(BodyTy::Unknown);
        };
        let Some(ret_ty) = &function_data.declaration.ret_ty else {
            return Ok(BodyTy::Unit);
        };

        Ok(match function_data.owner {
            BodyFunctionOwner::LocalImpl(impl_id) => {
                self.ty_from_type_ref_for_local_impl(ret_ty, impl_id, function_ref, receiver_ty)?
            }
        })
    }

    fn semantic_function_return_ty(
        &self,
        function_ref: FunctionRef,
        receiver_ty: Option<&BodyNominalTy>,
    ) -> Result<BodyTy, PackageStoreError> {
        let Some(function_data) = self.semantic_ir.function_data(function_ref)? else {
            return Ok(BodyTy::Unknown);
        };
        let Some(ret_ty) = function_data.signature.ret_ty() else {
            return Ok(BodyTy::Unit);
        };

        if receiver_ty.is_some() && type_ref_is_self(ret_ty) {
            return Ok(receiver_ty
                .cloned()
                .map(|ty| BodyTy::Nominal(vec![ty]))
                .unwrap_or(BodyTy::Unknown));
        }

        let subst = receiver_ty
            .map(|ty| {
                // Receiver type args and impl self args both contribute substitutions. For
                // `impl<U> Wrapper<U>`, this maps `U` to the known receiver argument.
                let mut subst = self.semantic_type_subst(ty)?;
                subst.extend(semantic_impl_self_subst(self.semantic_ir, function_ref, ty));
                Ok(subst)
            })
            .transpose()?
            .unwrap_or_default();
        self.type_path_resolver()
            .ty_from_type_ref_for_function_with_subst(ret_ty, function_ref, &subst)
    }

    fn ty_from_type_ref_for_local_impl(
        &self,
        ty: &TypeRef,
        impl_id: BodyImplId,
        function_ref: BodyFunctionRef,
        receiver_ty: Option<&BodyLocalNominalTy>,
    ) -> Result<BodyTy, PackageStoreError> {
        let Some(impl_data) = self.body.local_impl(impl_id) else {
            return Ok(BodyTy::Unknown);
        };

        if let TypeRef::Path(type_path) = ty {
            let path = Path::from_type_path(type_path);
            if path.is_self_type() {
                if let Some(receiver_ty) = receiver_ty {
                    return Ok(BodyTy::LocalNominal(vec![receiver_ty.clone()]));
                }
                return Ok(impl_data
                    .self_item
                    .map(|item| BodyTy::LocalNominal(vec![BodyLocalNominalTy::bare(item)]))
                    .unwrap_or(BodyTy::Unknown));
            }
        }

        let subst = receiver_ty
            .map(|ty| {
                // Receiver type args and impl self args both contribute substitutions. For
                // `impl<U> Wrapper<U>`, this maps `U` to the known receiver argument.
                let mut subst = self.local_type_subst(ty);
                subst.extend(local_impl_self_subst(self.body, function_ref, ty));
                subst
            })
            .unwrap_or_default();
        self.type_path_resolver()
            .ty_from_type_ref_in_scope_with_subst(ty, impl_data.scope, &subst)
    }

    fn resolve_local_name(
        &self,
        mut scope: ScopeId,
        name: &str,
        expr: ExprId,
    ) -> Option<BindingId> {
        loop {
            let scope_data = self.body.scope(scope)?;
            for binding in scope_data.bindings.iter().rev() {
                if binding.0 >= self.body.exprs[expr].visible_bindings {
                    continue;
                }

                let binding_data = self.body.binding(*binding)?;
                if binding_data.name.as_deref() == Some(name) {
                    return Some(*binding);
                }
            }

            scope = scope_data.parent?;
        }
    }

    fn call_ty(&self, callee: Option<ExprId>) -> Result<BodyTy, PackageStoreError> {
        let Some(callee) = callee else {
            return Ok(BodyTy::Unknown);
        };
        let callee_data = &self.body.exprs[callee];

        if matches!(
            callee_data.ty,
            BodyTy::Nominal(_) | BodyTy::SelfTy(_) | BodyTy::LocalNominal(_)
        ) {
            return Ok(callee_data.ty.clone());
        }

        // Ordinary calls use explicit return types only. Generic function inference remains
        // outside the current intentionally-small Body IR model.
        let mut return_tys = Vec::new();
        match &callee_data.resolution {
            BodyResolution::Item(defs) => {
                for def in defs {
                    let Some(function_ref) = self.function_ref_for_def(*def)? else {
                        continue;
                    };
                    push_unique(
                        &mut return_tys,
                        self.semantic_function_return_ty(function_ref, None)?,
                    );
                }
            }
            BodyResolution::Function(functions) => {
                for function in functions {
                    match function {
                        ResolvedFunctionRef::Semantic(function_ref) => {
                            push_unique(
                                &mut return_tys,
                                self.semantic_function_return_ty(*function_ref, None)?,
                            );
                        }
                        ResolvedFunctionRef::BodyLocal(function_ref) => {
                            push_unique(
                                &mut return_tys,
                                self.local_function_return_ty(*function_ref, None)?,
                            );
                        }
                    }
                }
            }
            BodyResolution::Local(_)
            | BodyResolution::LocalItem(_)
            | BodyResolution::Field(_)
            | BodyResolution::EnumVariant(_)
            | BodyResolution::Method(_)
            | BodyResolution::Unknown => {}
        }

        if return_tys.len() == 1 {
            Ok(return_tys.pop().expect("one return type should exist"))
        } else {
            Ok(BodyTy::Unknown)
        }
    }

    fn function_ref_for_def(&self, def: DefId) -> Result<Option<FunctionRef>, PackageStoreError> {
        let DefId::Local(local_def) = def else {
            return Ok(None);
        };
        self.semantic_ir.function_for_local_def(local_def)
    }
}

/// Resolves body value paths without mutating the body.
///
/// The main resolver uses this during the fixed-point pass, and analysis reuses it for cursor
/// queries over path prefixes. Keeping it read-only avoids cloning bodies just to answer
/// goto-definition/type-at for `Type::assoc` or `Enum::Variant` segments.
pub(super) struct BodyValuePathResolver<'query, 'db, 'body> {
    def_map: &'query DefMapReadTxn<'db>,
    semantic_ir: &'query SemanticIrReadTxn<'db>,
    semantic_index: Option<&'query SemanticResolutionIndex>,
    body_ref: BodyRef,
    body: &'body BodyData,
}

impl<'query, 'db, 'body> BodyValuePathResolver<'query, 'db, 'body> {
    pub(super) fn new(
        def_map: &'query DefMapReadTxn<'db>,
        semantic_ir: &'query SemanticIrReadTxn<'db>,
        semantic_index: Option<&'query SemanticResolutionIndex>,
        body_ref: BodyRef,
        body: &'body BodyData,
    ) -> Self {
        Self {
            def_map,
            semantic_ir,
            semantic_index,
            body_ref,
            body,
        }
    }

    pub(super) fn resolve_nonlocal_path_expr(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<(BodyResolution, BodyTy), PackageStoreError> {
        // Value paths can start with type-like names: tuple/unit struct constructors, body-local
        // item constructors, `Self`, and the prefix of associated paths all need type resolution
        // before falling back to ordinary module/DefMap lookup.
        match self.type_path_resolver().resolve_in_scope(scope, path)? {
            BodyTypePathResolution::BodyLocal(item_ref) => {
                return Ok((
                    BodyResolution::LocalItem(item_ref),
                    BodyTy::LocalNominal(vec![BodyLocalNominalTy::bare(item_ref)]),
                ));
            }
            BodyTypePathResolution::SelfType(types) => {
                return Ok((
                    BodyResolution::Unknown,
                    BodyTy::SelfTy(types.into_iter().map(BodyNominalTy::bare).collect()),
                ));
            }
            BodyTypePathResolution::TypeDefs(_)
            | BodyTypePathResolution::Traits(_)
            | BodyTypePathResolution::Unknown => {}
        }

        if let Some((prefix, last_segment)) = split_associated_path(path) {
            if let Some((resolution, ty)) =
                self.resolve_associated_path(scope, &prefix, last_segment)?
            {
                return Ok((resolution, ty));
            }
        }

        let result = self.def_map.resolve_path(self.body.owner_module, path)?;
        let ty = self.nominal_ty_from_defs(&result.resolved)?;
        Ok((BodyResolution::Item(result.resolved), ty))
    }

    fn type_path_resolver(&self) -> BodyTypePathResolver<'_, 'db, '_> {
        BodyTypePathResolver::new(self.def_map, self.semantic_ir, self.body_ref, self.body)
    }

    fn resolve_associated_path(
        &self,
        scope: ScopeId,
        prefix: &Path,
        last_segment: &str,
    ) -> Result<Option<(BodyResolution, BodyTy)>, PackageStoreError> {
        // Associated value paths are resolved as "type prefix + value member". This keeps
        // `Action::Start` distinct from a module path while also handling `Widget::new` through
        // the same type-substitution rules used by method calls.
        let prefix_resolution = self.type_path_resolver().resolve_in_scope(scope, prefix)?;
        let prefix_ty = type_path_resolution_to_body_ty(prefix_resolution);

        let mut variants = Vec::new();
        let mut variant_tys = Vec::new();
        for nominal_ty in prefix_ty.nominal_tys() {
            if !matches!(nominal_ty.def.id, TypeDefId::Enum(_)) {
                continue;
            }
            let Some(variant_ref) = self
                .semantic_ir
                .enum_variant_ref_for_type_def(nominal_ty.def, last_segment)?
            else {
                continue;
            };
            push_unique(&mut variants, variant_ref);
            push_unique(&mut variant_tys, BodyTy::Nominal(vec![nominal_ty.clone()]));
        }

        if !variants.is_empty() {
            let ty = unique_ty_or_unknown(variant_tys);
            return Ok(Some((BodyResolution::EnumVariant(variants), ty)));
        }

        // Inherent associated functions are exact candidates. Trait-associated functions are kept
        // deliberately optimistic, following the same "prefer useful candidates over false
        // negatives" policy as dot completion.
        let mut functions = Vec::new();
        for local_ty in prefix_ty.local_nominals() {
            for function_ref in self.local_associated_functions_for_type(local_ty)? {
                let Some(function_data) = self.body.local_function(function_ref.function) else {
                    continue;
                };
                if function_data.name == last_segment && !function_data.has_self_receiver() {
                    push_unique(&mut functions, ResolvedFunctionRef::BodyLocal(function_ref));
                }
            }
        }
        for nominal_ty in prefix_ty.nominal_tys() {
            for function_ref in self.semantic_associated_functions_for_type(nominal_ty)? {
                let Some(function_data) = self.semantic_ir.function_data(function_ref)? else {
                    continue;
                };
                if function_data.name == last_segment && !function_data.has_self_receiver() {
                    push_unique(&mut functions, ResolvedFunctionRef::Semantic(function_ref));
                }
            }
        }

        Ok((!functions.is_empty())
            .then_some((BodyResolution::Function(functions), BodyTy::Unknown)))
    }

    fn local_associated_functions_for_type(
        &self,
        ty: &BodyLocalNominalTy,
    ) -> Result<Vec<BodyFunctionRef>, PackageStoreError> {
        if ty.item.body != self.body_ref {
            return Ok(Vec::new());
        }

        let mut functions = Vec::new();
        for function in self
            .body
            .inherent_functions_for_local_type(self.body_ref, ty.item)
        {
            if local_function_applies_to_receiver(
                self.def_map,
                self.semantic_ir,
                self.body_ref,
                self.body,
                function,
                ty,
            )? {
                functions.push(function);
            }
        }
        Ok(functions)
    }

    fn semantic_associated_functions_for_type(
        &self,
        ty: &BodyNominalTy,
    ) -> Result<Vec<FunctionRef>, PackageStoreError> {
        let mut functions = Vec::new();
        let inherent_functions = match self.semantic_index {
            Some(index) => index.inherent_functions_for_type(self.semantic_ir, ty.def)?,
            None => self.semantic_ir.inherent_functions_for_type(ty.def)?,
        };

        for function in inherent_functions {
            if semantic_function_applies_to_receiver(self.def_map, self.semantic_ir, function, ty)?
            {
                functions.push(function);
            }
        }

        for (function, _) in semantic_trait_function_candidates_for_receiver(
            self.semantic_index,
            self.def_map,
            self.semantic_ir,
            ty,
            None,
        )? {
            push_unique(&mut functions, function);
        }
        Ok(functions)
    }

    fn nominal_ty_from_defs(&self, defs: &[DefId]) -> Result<BodyTy, PackageStoreError> {
        let mut type_defs = Vec::new();
        for def in defs {
            let DefId::Local(local_def) = def else {
                continue;
            };
            let Some(type_def) = self.semantic_ir.type_def_for_local_def(*local_def)? else {
                continue;
            };
            push_unique(&mut type_defs, type_def);
        }

        Ok(if type_defs.is_empty() {
            BodyTy::Unknown
        } else {
            BodyTy::Nominal(type_defs.into_iter().map(BodyNominalTy::bare).collect())
        })
    }
}

fn split_associated_path(path: &Path) -> Option<(Path, &str)> {
    if path.segments.len() < 2 {
        return None;
    }

    let PathSegment::Name(last_segment) = path.segments.last()? else {
        return None;
    };

    Some((
        Path {
            absolute: path.absolute,
            segments: path.segments[..path.segments.len() - 1].to_vec(),
        },
        last_segment.as_str(),
    ))
}

fn type_path_resolution_to_body_ty(resolution: BodyTypePathResolution) -> BodyTy {
    match resolution {
        BodyTypePathResolution::BodyLocal(item) => {
            BodyTy::LocalNominal(vec![BodyLocalNominalTy::bare(item)])
        }
        BodyTypePathResolution::SelfType(types) => {
            BodyTy::SelfTy(types.into_iter().map(BodyNominalTy::bare).collect())
        }
        BodyTypePathResolution::TypeDefs(types) => {
            BodyTy::Nominal(types.into_iter().map(BodyNominalTy::bare).collect())
        }
        BodyTypePathResolution::Traits(_) | BodyTypePathResolution::Unknown => BodyTy::Unknown,
    }
}

fn unique_ty_or_unknown(mut tys: Vec<BodyTy>) -> BodyTy {
    if tys.len() == 1 {
        tys.pop().expect("one type should exist")
    } else {
        BodyTy::Unknown
    }
}
