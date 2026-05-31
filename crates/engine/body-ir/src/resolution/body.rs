//! Main body-resolution pass.
//!
//! This module walks lowered bodies and fills resolution/type slots on bindings and expressions.
//! Specialized helpers live in sibling modules so this file can read like the pass itself.

use rg_def_map::{DefMapReadTxn, Path, PathSegment};
use rg_ir_model::{
    AssocItemId, BindingId, BodyRef, ConstRef, DefId, DefMapRef, ExprId, FunctionRef, ImplRef,
    ItemOwner, ModuleId, ModuleRef, ResolvedDeclarationRef, ScopeId, SemanticDeclarationRef,
    SemanticItemRef, StaticRef, TypeDefId,
};
use rg_item_tree::FieldKey;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::SemanticIrReadTxn;
use rg_ty::{IndexedNominalTy, IndexedTy, IndexedTyExt, IndexedTyRepr, IndexedTypeSubst};

use crate::{
    ir::body::BodyData,
    ir::expr::{ExprKind, ExprUnaryOp, ExprWrapperKind},
    ir::resolved::{BodyResolution, BodyTypePathResolution},
    ir::stmt::{BindingKind, BodySelfParamKind},
};

use super::{
    SemanticResolutionIndex,
    autoderef::{BodyAutoderef, BodyAutoderefMode},
    def_map_lookup::BodyDefMapLookup,
    impl_match::BodyImplMatcher,
    item_query::BodyItemQuery,
    method::{
        semantic_function_applies_to_receiver, semantic_trait_function_candidates_for_receiver,
    },
    normalize::IndexedTyNormalizer,
    pat::PatternTypePropagator,
    push_unique,
    ty::{subst_from_generics, type_ref_is_self},
    type_path::BodyTypePathResolver,
};

pub(crate) struct BodyResolver<'query, 'db, 'body> {
    def_map_txn: &'query DefMapReadTxn<'db>,
    semantic_ir_txn: &'query SemanticIrReadTxn<'db>,
    semantic_index: &'query SemanticResolutionIndex,
    body_ref: BodyRef,
    body: &'body mut BodyData,
}

impl<'query, 'db, 'body> BodyResolver<'query, 'db, 'body> {
    pub(crate) fn new(
        def_map_txn: &'query DefMapReadTxn<'db>,
        semantic_ir_txn: &'query SemanticIrReadTxn<'db>,
        semantic_index: &'query SemanticResolutionIndex,
        body_ref: BodyRef,
        body: &'body mut BodyData,
    ) -> Self {
        Self {
            def_map_txn,
            semantic_ir_txn,
            semantic_index,
            body_ref,
            body,
        }
    }

    fn type_path_resolver(&self) -> BodyTypePathResolver<'_, 'db, '_> {
        BodyTypePathResolver::new(
            self.def_map_txn,
            self.semantic_ir_txn,
            self.body_ref,
            self.body,
        )
    }

    fn autoderef(&self) -> BodyAutoderef<'_, 'db> {
        BodyAutoderef::with_index(self.def_map_txn, self.semantic_ir_txn, self.semantic_index)
    }

    fn semantic_impl_matcher(&self) -> BodyImplMatcher<'_, 'db> {
        BodyImplMatcher::new(self.def_map_txn, self.semantic_ir_txn)
    }

    fn item_query(&self) -> BodyItemQuery<'_, 'db, '_> {
        BodyItemQuery::new(self.semantic_ir_txn, self.body_ref, self.body)
    }

    pub(crate) fn resolve(&mut self) -> Result<(), PackageStoreError> {
        self.resolve_body_item_store_impls()?;
        self.resolve_bindings()?;

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
                self.def_map_txn,
                self.semantic_ir_txn,
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

    fn resolve_body_item_store_impls(&mut self) -> Result<(), PackageStoreError> {
        let Some(item_store) = self.body.body_item_store() else {
            return Ok(());
        };

        let impl_headers = item_store
            .impls_with_refs()
            .map(|(impl_ref, impl_data)| (impl_ref.id, impl_data.owner, impl_data.self_ty.clone()))
            .collect::<Vec<_>>();

        let mut resolved_headers = Vec::new();
        for (impl_id, owner, self_ty) in impl_headers {
            if owner.origin != DefMapRef::Body(self.body_ref) {
                continue;
            }

            let scope = ScopeId(owner.module.0);
            if self.body.scope(scope).is_none() {
                continue;
            }

            let ty = self
                .type_path_resolver()
                .ty_from_type_ref_in_scope(&self_ty, scope)?;
            let mut resolved_self_tys = Vec::new();
            for nominal in ty.as_nominals() {
                push_unique(&mut resolved_self_tys, nominal.def);
            }
            resolved_headers.push((impl_id, resolved_self_tys));
        }

        let Some(item_store) = self.body.body_item_store.as_mut() else {
            return Ok(());
        };
        for (impl_id, resolved_self_tys) in resolved_headers {
            if let Some(impl_data) = item_store.impls_mut().get_mut(impl_id) {
                impl_data.resolved_self_tys = resolved_self_tys;
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

    fn binding_ty(&self, binding: BindingId) -> Result<IndexedTy, PackageStoreError> {
        let binding_data = &self.body.bindings[binding];
        if let Some(annotation) = &binding_data.annotation {
            return self
                .type_path_resolver()
                .ty_from_type_ref_in_scope(annotation, binding_data.scope);
        }

        if let BindingKind::SelfParam(kind) = binding_data.kind
            && binding_data.name.as_deref() == Some("self")
        {
            let self_tys = self
                .type_path_resolver()
                .self_tys_for_function(self.body.owner)?;
            if !self_tys.is_empty() {
                let ty = IndexedTyRepr::self_ty(
                    self_tys.into_iter().map(IndexedNominalTy::bare).collect(),
                );
                return Ok(match kind {
                    BodySelfParamKind::Value => ty,
                    BodySelfParamKind::Reference { mutability } => {
                        IndexedTy::reference(mutability, ty)
                    }
                    BodySelfParamKind::Explicit => IndexedTy::Unknown,
                });
            }
        }

        Ok(IndexedTy::Unknown)
    }

    fn resolve_expr(&mut self, expr: ExprId) -> Result<bool, PackageStoreError> {
        let old_resolution = self.body.exprs[expr].resolution.clone();
        let old_ty = self.body.exprs[expr].ty.clone();
        let kind = self.body.exprs[expr].kind.clone();

        match kind {
            ExprKind::Path { path } => {
                let (resolution, ty) = match path.as_def_map_path() {
                    Some(path) => self.resolve_path_expr(expr, &path)?,
                    None => (BodyResolution::Unknown, IndexedTy::Unknown),
                };
                let data = &mut self.body.exprs[expr];
                data.resolution = resolution;
                data.ty = ty;
            }
            ExprKind::Call { callee, .. } => {
                self.body.exprs[expr].ty = self.call_ty(callee)?;
            }
            ExprKind::Tuple { fields } if fields.is_empty() => {
                self.body.exprs[expr].ty = IndexedTy::Unit;
            }
            ExprKind::Cast { ty: Some(ty), .. } => {
                self.body.exprs[expr].ty = self
                    .type_path_resolver()
                    .ty_from_type_ref_in_scope(&ty, self.body.exprs[expr].scope)?;
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
                    IndexedTy::Unknown
                };
            }
            ExprKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                self.body.exprs[expr].ty = match else_branch {
                    Some(else_branch) => {
                        let mut branch_tys = Vec::new();
                        if let Some(then_branch) = then_branch {
                            push_unique(&mut branch_tys, self.body.exprs[then_branch].ty.clone());
                        }
                        push_unique(&mut branch_tys, self.body.exprs[else_branch].ty.clone());

                        if branch_tys.len() == 1 {
                            branch_tys.pop().expect("one branch type should exist")
                        } else {
                            IndexedTy::Unknown
                        }
                    }
                    None => IndexedTy::Unit,
                };
            }
            ExprKind::Block { tail, .. } => {
                self.body.exprs[expr].ty = tail
                    .map(|tail| self.body.exprs[tail].ty.clone())
                    .unwrap_or(IndexedTy::Unit);
            }
            ExprKind::Field { base, field, .. } => {
                let (resolution, ty) = self.resolve_field_expr(base, field.as_ref())?;
                let data = &mut self.body.exprs[expr];
                data.resolution = resolution;
                data.ty = ty;
            }
            ExprKind::Record { path, .. } => {
                let (resolution, ty) = match path.as_ref().and_then(|path| path.as_def_map_path()) {
                    Some(path) => {
                        self.resolve_record_expr_path(self.body.exprs[expr].scope, &path)?
                    }
                    None => (BodyResolution::Unknown, IndexedTy::Unknown),
                };
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
            ExprKind::Unary {
                op: Some(ExprUnaryOp::Deref),
                expr: Some(inner),
            } => {
                self.body.exprs[expr].ty = self.explicit_deref_ty(inner)?;
            }
            ExprKind::While { .. } | ExprKind::For { .. } => {
                self.body.exprs[expr].ty = IndexedTy::Unit;
            }
            ExprKind::Assign { .. } => {
                self.body.exprs[expr].ty = IndexedTy::Unit;
            }
            ExprKind::Break { .. } | ExprKind::Continue { .. } => {
                self.body.exprs[expr].ty = IndexedTy::Never;
            }
            ExprKind::Yeet { .. } | ExprKind::Become { .. } => {
                self.body.exprs[expr].ty = IndexedTy::Never;
            }
            ExprKind::Let { .. }
            | ExprKind::Closure { .. }
            | ExprKind::Loop { .. }
            | ExprKind::Tuple { .. }
            | ExprKind::Array { .. }
            | ExprKind::RepeatArray { .. }
            | ExprKind::Index { .. }
            | ExprKind::Range { .. }
            | ExprKind::Cast { ty: None, .. }
            | ExprKind::Unary { .. }
            | ExprKind::Binary { .. }
            | ExprKind::Literal { .. }
            | ExprKind::Underscore
            | ExprKind::Yield { .. }
            | ExprKind::Unknown { .. } => {}
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
    ) -> Result<(BodyResolution, IndexedTy), PackageStoreError> {
        let scope = self.body.exprs[expr].scope;
        let visible_bindings = self.body.exprs[expr].visible_bindings;
        BodyValuePathResolver::new(
            self.def_map_txn,
            self.semantic_ir_txn,
            Some(self.semantic_index),
            self.body_ref,
            self.body,
        )
        .resolve_path_expr(scope, path, Some(visible_bindings))
    }

    pub(super) fn resolve_nonlocal_path_expr(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<(BodyResolution, IndexedTy), PackageStoreError> {
        BodyValuePathResolver::new(
            self.def_map_txn,
            self.semantic_ir_txn,
            Some(self.semantic_index),
            self.body_ref,
            self.body,
        )
        .resolve_nonlocal_path_expr(scope, path)
    }

    fn resolve_record_expr_path(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<(BodyResolution, IndexedTy), PackageStoreError> {
        match self.type_path_resolver().resolve_in_scope(scope, path)? {
            BodyTypePathResolution::SelfType(types) => {
                return Ok((
                    BodyResolution::Unknown,
                    IndexedTyRepr::self_ty(types.into_iter().map(IndexedNominalTy::bare).collect()),
                ));
            }
            BodyTypePathResolution::TypeDefs(types) => {
                let types = types
                    .into_iter()
                    .filter(|ty| ty.origin == DefMapRef::Body(self.body_ref))
                    .collect::<Vec<_>>();
                if !types.is_empty() {
                    return Ok((
                        BodyResolution::Declaration(
                            types
                                .iter()
                                .copied()
                                .map(ResolvedDeclarationRef::from)
                                .collect(),
                        ),
                        IndexedTyRepr::nominal(
                            types.into_iter().map(IndexedNominalTy::bare).collect(),
                        ),
                    ));
                }
            }
            BodyTypePathResolution::Primitive(_)
            | BodyTypePathResolution::TypeAliases(_)
            | BodyTypePathResolution::Traits(_)
            | BodyTypePathResolution::Unknown => {}
        }

        self.resolve_nonlocal_path_expr(scope, path)
    }

    fn resolve_field_expr(
        &self,
        base: Option<ExprId>,
        field: Option<&FieldKey>,
    ) -> Result<(BodyResolution, IndexedTy), PackageStoreError> {
        let (Some(base), Some(field)) = (base, field) else {
            return Ok((BodyResolution::Unknown, IndexedTy::Unknown));
        };

        let item_query = self.item_query();
        let mut current_depth = None;
        let mut fields = Vec::new();
        let mut field_tys = Vec::new();

        for candidate in self
            .autoderef()
            .candidates(BodyAutoderefMode::FieldLookup, &self.body.exprs[base].ty)
        {
            let candidate = candidate?;
            // Autoderef yields candidates by depth. Resolve only after the whole matching depth is
            // collected, so same-depth alternatives produce ambiguity instead of order dependence.
            if current_depth.is_some_and(|depth| depth != candidate.depth()) && !fields.is_empty() {
                let ty = if field_tys.len() == 1 {
                    field_tys.pop().expect("one field type should exist")
                } else {
                    IndexedTy::Unknown
                };
                return Ok((
                    BodyResolution::Field(
                        fields
                            .into_iter()
                            .map(ResolvedDeclarationRef::from)
                            .collect(),
                    ),
                    ty,
                ));
            }
            current_depth = Some(candidate.depth());

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
                    .type_path_resolver()
                    .ty_from_type_ref_in_module_with_subst(
                        &field_data.field.ty,
                        field_data.owner_module,
                        &subst,
                    )?;
                push_unique(&mut field_tys, field_ty);
            }
        }

        if !fields.is_empty() {
            let ty = if field_tys.len() == 1 {
                field_tys.pop().expect("one field type should exist")
            } else {
                IndexedTy::Unknown
            };
            return Ok((
                BodyResolution::Field(
                    fields
                        .into_iter()
                        .map(ResolvedDeclarationRef::from)
                        .collect(),
                ),
                ty,
            ));
        }

        Ok((BodyResolution::Unknown, IndexedTy::Unknown))
    }

    fn resolve_method_call_expr(
        &self,
        receiver: Option<ExprId>,
        method_name: &str,
    ) -> Result<(BodyResolution, IndexedTy), PackageStoreError> {
        let Some(receiver) = receiver else {
            return Ok((BodyResolution::Unknown, IndexedTy::Unknown));
        };

        let receiver_ty = &self.body.exprs[receiver].ty;
        let item_query = self.item_query();

        // Method lookup is intentionally shallow: nominal type plus lightweight impl-argument
        // matching gives useful candidates without modeling the full trait solver.
        let mut current_depth = None;
        let mut functions = Vec::new();
        let mut return_tys = Vec::new();

        for candidate in self
            .autoderef()
            .candidates(BodyAutoderefMode::MethodReceiver, receiver_ty)
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
                    IndexedTy::Unknown
                };
                return Ok((
                    BodyResolution::Method(
                        functions
                            .into_iter()
                            .map(ResolvedDeclarationRef::from)
                            .collect(),
                    ),
                    ty,
                ));
            }
            current_depth = Some(candidate.depth());

            for nominal_ty in candidate.ty().as_nominals() {
                for function_ref in self.semantic_functions_for_type(nominal_ty, method_name)? {
                    let Some(function_data) = item_query.function_data(function_ref)? else {
                        continue;
                    };
                    if function_data.name != method_name || !function_data.has_self_receiver() {
                        continue;
                    }

                    push_unique(&mut functions, function_ref);
                    push_unique(
                        &mut return_tys,
                        self.semantic_function_return_ty(function_ref, Some(nominal_ty))?,
                    );
                }
            }
        }

        if !functions.is_empty() {
            let ty = if return_tys.len() == 1 {
                return_tys.pop().expect("one return type should exist")
            } else {
                IndexedTy::Unknown
            };
            return Ok((
                BodyResolution::Method(
                    functions
                        .into_iter()
                        .map(ResolvedDeclarationRef::from)
                        .collect(),
                ),
                ty,
            ));
        }

        Ok((BodyResolution::Unknown, IndexedTy::Unknown))
    }

    fn resolve_wrapper_expr(
        &self,
        kind: ExprWrapperKind,
        inner: Option<ExprId>,
    ) -> (BodyResolution, IndexedTy) {
        let Some(inner) = inner else {
            return (BodyResolution::Unknown, IndexedTy::Unknown);
        };
        let inner_data = &self.body.exprs[inner];
        let ty = IndexedTyNormalizer::new(self.semantic_ir_txn, self.body_ref, self.body)
            .ty_for_wrapper(kind, inner_data.ty.clone());
        let resolution = if matches!(kind, ExprWrapperKind::Paren) {
            inner_data.resolution.clone()
        } else {
            BodyResolution::Unknown
        };

        (resolution, ty)
    }

    fn explicit_deref_ty(&self, inner: ExprId) -> Result<IndexedTy, PackageStoreError> {
        let mut candidates = Vec::new();
        for candidate in self
            .autoderef()
            .candidates(BodyAutoderefMode::ExplicitDeref, &self.body.exprs[inner].ty)
        {
            push_unique(&mut candidates, candidate?.ty().clone());
        }

        Ok(if candidates.len() == 1 {
            candidates
                .pop()
                .expect("one explicit deref candidate should exist")
        } else {
            IndexedTy::Unknown
        })
    }

    fn semantic_functions_for_type(
        &self,
        ty: &IndexedNominalTy,
        method_name: &str,
    ) -> Result<Vec<FunctionRef>, PackageStoreError> {
        let mut functions = Vec::new();
        if ty.def.origin == DefMapRef::Body(self.body_ref) {
            let item_query = self.item_query();
            for function in item_query.inherent_functions_for_type(ty.def)? {
                let Some(function_data) = item_query.function_data(function)? else {
                    continue;
                };
                if function_data.name != method_name {
                    continue;
                }
                if self.item_store_function_applies_to_receiver(
                    function,
                    function_data.owner,
                    ty,
                )? {
                    functions.push(function);
                }
            }
            return Ok(functions);
        }

        for function in self
            .semantic_index
            .inherent_functions_for_type_and_name(ty.def, method_name)
            .to_vec()
        {
            if semantic_function_applies_to_receiver(
                self.def_map_txn,
                self.semantic_ir_txn,
                function,
                ty,
            )? {
                functions.push(function);
            }
        }

        for (function, _) in semantic_trait_function_candidates_for_receiver(
            Some(self.semantic_index),
            self.def_map_txn,
            self.semantic_ir_txn,
            ty,
            Some(method_name),
        )? {
            push_unique(&mut functions, function);
        }
        Ok(functions)
    }

    fn item_store_function_applies_to_receiver(
        &self,
        function_ref: FunctionRef,
        owner: ItemOwner,
        receiver_ty: &IndexedNominalTy,
    ) -> Result<bool, PackageStoreError> {
        let ItemOwner::Impl(impl_id) = owner else {
            return Ok(true);
        };
        let impl_ref = ImplRef {
            origin: function_ref.origin,
            id: impl_id,
        };
        let item_query = self.item_query();
        let Some(impl_data) = item_query.impl_data(impl_ref)? else {
            return Ok(false);
        };
        if impl_data.trait_ref.is_some() {
            return Ok(false);
        }

        self.semantic_impl_matcher()
            .impl_applies_to_receiver(impl_ref, impl_data, receiver_ty)
    }

    fn impl_self_subst_for_function(
        &self,
        function_ref: FunctionRef,
        owner: ItemOwner,
        receiver_ty: &IndexedNominalTy,
    ) -> Result<IndexedTypeSubst, PackageStoreError> {
        let ItemOwner::Impl(impl_id) = owner else {
            return Ok(IndexedTypeSubst::new());
        };
        let item_query = self.item_query();
        let Some(impl_data) = item_query.impl_data(ImplRef {
            origin: function_ref.origin,
            id: impl_id,
        })?
        else {
            return Ok(IndexedTypeSubst::new());
        };

        Ok(self
            .semantic_impl_matcher()
            .impl_self_subst_for_impl(impl_data, receiver_ty))
    }

    fn semantic_type_subst(
        &self,
        ty: &IndexedNominalTy,
    ) -> Result<IndexedTypeSubst, PackageStoreError> {
        Ok(self
            .item_query()
            .generic_params_for_type_def(ty.def)?
            .map(|generics| subst_from_generics(generics, &ty.args))
            .unwrap_or_else(IndexedTypeSubst::new))
    }

    fn semantic_function_return_ty(
        &self,
        function_ref: FunctionRef,
        receiver_ty: Option<&IndexedNominalTy>,
    ) -> Result<IndexedTy, PackageStoreError> {
        let item_query = self.item_query();
        let Some(function_data) = item_query.function_data(function_ref)? else {
            return Ok(IndexedTy::Unknown);
        };
        let Some(ret_ty) = function_data.signature.ret_ty() else {
            return Ok(IndexedTy::Unit);
        };

        if receiver_ty.is_some() && type_ref_is_self(ret_ty) {
            return Ok(receiver_ty
                .cloned()
                .map(|ty| IndexedTyRepr::nominal(vec![ty]))
                .unwrap_or(IndexedTy::Unknown));
        }

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
        self.type_path_resolver()
            .ty_from_type_ref_for_function_with_subst(ret_ty, function_ref, &subst)
    }

    fn call_ty(&self, callee: Option<ExprId>) -> Result<IndexedTy, PackageStoreError> {
        let Some(callee) = callee else {
            return Ok(IndexedTy::Unknown);
        };
        let callee_data = &self.body.exprs[callee];

        if matches!(
            callee_data.ty,
            IndexedTy::Repr(IndexedTyRepr::Nominal(_) | IndexedTyRepr::SelfTy(_))
        ) {
            return Ok(callee_data.ty.clone());
        }

        // Ordinary calls use explicit return types only. Generic function inference remains
        // outside the current intentionally-small Body IR model.
        let mut return_tys = Vec::new();
        match &callee_data.resolution {
            BodyResolution::Declaration(declarations) | BodyResolution::Function(declarations) => {
                for declaration in declarations {
                    self.push_return_ty_for_declaration(*declaration, &mut return_tys)?;
                }
            }
            BodyResolution::Local(_)
            | BodyResolution::Field(_)
            | BodyResolution::EnumVariant(_)
            | BodyResolution::Method(_)
            | BodyResolution::Unknown => {}
        }

        if return_tys.len() == 1 {
            Ok(return_tys.pop().expect("one return type should exist"))
        } else {
            Ok(IndexedTy::Unknown)
        }
    }

    fn push_return_ty_for_declaration(
        &self,
        declaration: ResolvedDeclarationRef,
        return_tys: &mut Vec<IndexedTy>,
    ) -> Result<(), PackageStoreError> {
        match declaration {
            ResolvedDeclarationRef::Def(def) => {
                let Some(function_ref) = self.function_ref_for_def(def)? else {
                    return Ok(());
                };
                push_unique(
                    return_tys,
                    self.semantic_function_return_ty(function_ref, None)?,
                );
            }
            ResolvedDeclarationRef::Semantic(SemanticDeclarationRef::Item(
                SemanticItemRef::Function(function_ref),
            )) => {
                push_unique(
                    return_tys,
                    self.semantic_function_return_ty(function_ref, None)?,
                );
            }
            ResolvedDeclarationRef::Semantic(
                SemanticDeclarationRef::Item(
                    SemanticItemRef::TypeDef(_)
                    | SemanticItemRef::Trait(_)
                    | SemanticItemRef::Impl(_)
                    | SemanticItemRef::TypeAlias(_)
                    | SemanticItemRef::Const(_)
                    | SemanticItemRef::Static(_),
                )
                | SemanticDeclarationRef::Field(_)
                | SemanticDeclarationRef::EnumVariant(_),
            ) => {}
        }

        Ok(())
    }

    fn function_ref_for_def(&self, def: DefId) -> Result<Option<FunctionRef>, PackageStoreError> {
        let DefId::Local(local_def) = def else {
            return Ok(None);
        };
        Ok(
            match self
                .semantic_ir_txn
                .semantic_item_for_local_def(local_def)?
            {
                Some(SemanticItemRef::Function(function)) => Some(function),
                Some(_) | None => None,
            },
        )
    }
}

/// Resolves body value paths without mutating the body.
///
/// The main resolver uses this during the fixed-point pass, and analysis reuses it for cursor
/// queries over path prefixes. Keeping it read-only avoids cloning bodies just to answer
/// goto-definition/type-at for `Type::assoc` or `Enum::Variant` segments.
pub(crate) struct BodyValuePathResolver<'query, 'db, 'body> {
    def_map: &'query DefMapReadTxn<'db>,
    semantic_ir: &'query SemanticIrReadTxn<'db>,
    semantic_index: Option<&'query SemanticResolutionIndex>,
    body_ref: BodyRef,
    body: &'body BodyData,
}

/// One declaration that can satisfy an unqualified value path inside a body scope.
///
/// Rust shares bindings and item-like declarations in the value namespace. Keeping them under one
/// enum lets lookup stay scope-ordered instead of accidentally searching one category through every
/// parent scope before the next category.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BodyValueName {
    Binding(BindingId),
    SemanticItems(Vec<SemanticItemRef>),
}

impl<'query, 'db, 'body> BodyValuePathResolver<'query, 'db, 'body> {
    pub(crate) fn new(
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

    fn type_path_resolver(&self) -> BodyTypePathResolver<'_, 'db, 'body> {
        BodyTypePathResolver::new(self.def_map, self.semantic_ir, self.body_ref, self.body)
    }

    fn semantic_impl_matcher(&self) -> BodyImplMatcher<'_, 'db> {
        BodyImplMatcher::new(self.def_map, self.semantic_ir)
    }

    fn item_query(&self) -> BodyItemQuery<'_, 'db, '_> {
        BodyItemQuery::new(self.semantic_ir, self.body_ref, self.body)
    }

    pub(crate) fn resolve_nonlocal_path_expr(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<(BodyResolution, IndexedTy), PackageStoreError> {
        self.resolve_path_expr(scope, path, None)
    }

    pub(super) fn resolve_path_expr(
        &self,
        scope: ScopeId,
        path: &Path,
        visible_bindings: Option<usize>,
    ) -> Result<(BodyResolution, IndexedTy), PackageStoreError> {
        if let Some(name) = path.single_name() {
            if let Some((resolution, ty)) =
                self.resolve_single_segment_value_name(scope, name, visible_bindings)?
            {
                return Ok((resolution, ty));
            }
        }

        // Value paths can start with type-like names: tuple/unit struct constructors, `Self`, and
        // the prefix of associated paths all need type resolution before falling back to ordinary
        // module/DefMap lookup.
        match self.type_path_resolver().resolve_in_scope(scope, path)? {
            BodyTypePathResolution::SelfType(types) => {
                return Ok((
                    BodyResolution::Unknown,
                    IndexedTyRepr::self_ty(types.into_iter().map(IndexedNominalTy::bare).collect()),
                ));
            }
            BodyTypePathResolution::TypeDefs(types) => {
                let mut constructors = Vec::new();
                for type_def in types
                    .into_iter()
                    .filter(|ty| ty.origin == DefMapRef::Body(self.body_ref))
                {
                    if self.item_query().type_def_has_value_constructor(type_def)? {
                        push_unique(&mut constructors, type_def);
                    }
                }

                if !constructors.is_empty() {
                    return Ok((
                        BodyResolution::Declaration(
                            constructors
                                .iter()
                                .copied()
                                .map(ResolvedDeclarationRef::from)
                                .collect(),
                        ),
                        IndexedTyRepr::nominal(
                            constructors
                                .into_iter()
                                .map(IndexedNominalTy::bare)
                                .collect(),
                        ),
                    ));
                }
            }
            BodyTypePathResolution::Primitive(_)
            | BodyTypePathResolution::TypeAliases(_)
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

        if path.single_name().is_none()
            && let Some((resolution, ty)) =
                self.resolve_body_value_path_from_def_map(scope, path)?
        {
            return Ok((resolution, ty));
        }

        let result = self.def_map.resolve_path(self.body.owner_module, path)?;
        let ty = self.nominal_ty_from_defs(&result.resolved)?;
        Ok((
            BodyResolution::Declaration(
                result
                    .resolved
                    .into_iter()
                    .map(ResolvedDeclarationRef::from)
                    .collect(),
            ),
            ty,
        ))
    }

    fn resolve_single_segment_value_name(
        &self,
        start_scope: ScopeId,
        name: &str,
        visible_bindings: Option<usize>,
    ) -> Result<Option<(BodyResolution, IndexedTy)>, PackageStoreError> {
        // Value lookup is scope-ordered: an inner const/function shadows an outer binding just as
        // surely as an inner binding shadows an outer item.
        let from = ModuleRef {
            origin: DefMapRef::Body(self.body_ref),
            module: ModuleId(start_scope.0),
        };
        let mut scope = Some(start_scope);
        while let Some(scope_id) = scope {
            let Some(scope_data) = self.body.scope(scope_id) else {
                return Ok(None);
            };

            if let Some(visible_bindings) = visible_bindings {
                for binding in scope_data.bindings.iter().rev() {
                    if binding.0 >= visible_bindings {
                        continue;
                    }

                    let Some(binding_data) = self.body.binding(*binding) else {
                        continue;
                    };
                    if binding_data.name.as_deref() == Some(name) {
                        return self.value_name_resolution(BodyValueName::Binding(*binding));
                    }
                }
            }

            let module = ModuleRef {
                origin: DefMapRef::Body(self.body_ref),
                module: ModuleId(scope_id.0),
            };
            let Some(def_map) = self.body.body_def_map() else {
                scope = scope_data.parent;
                continue;
            };
            let defs = BodyDefMapLookup::new(def_map)
                .resolve_name_in_value_namespace_at_module(from, module, name);
            let value_name = BodyValueName::SemanticItems(self.semantic_items_for_defs(defs)?);
            if let Some(resolution) = self.value_name_resolution(value_name)? {
                return Ok(Some(resolution));
            }

            scope = scope_data.parent;
        }

        Ok(None)
    }

    fn resolve_body_value_path_from_def_map(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<Option<(BodyResolution, IndexedTy)>, PackageStoreError> {
        let Some(def_map) = self.body.body_def_map() else {
            return Ok(None);
        };

        let from = ModuleRef {
            origin: DefMapRef::Body(self.body_ref),
            module: ModuleId(scope.0),
        };
        let defs = BodyDefMapLookup::new(def_map)
            .resolve_path_in_value_namespace(from, path)
            .resolved;
        self.value_name_resolution(BodyValueName::SemanticItems(
            self.semantic_items_for_defs(defs)?,
        ))
    }

    fn semantic_items_for_defs(
        &self,
        defs: Vec<DefId>,
    ) -> Result<Vec<SemanticItemRef>, PackageStoreError> {
        let mut items = Vec::new();
        for def in defs {
            let DefId::Local(local_def) = def else {
                continue;
            };
            let Some(item) = self.item_query().semantic_item_for_local_def(local_def)? else {
                continue;
            };
            if matches!(
                item,
                SemanticItemRef::Function(_)
                    | SemanticItemRef::Const(_)
                    | SemanticItemRef::Static(_)
            ) {
                push_unique(&mut items, item);
            }
        }

        Ok(items)
    }

    fn value_name_resolution(
        &self,
        value_name: BodyValueName,
    ) -> Result<Option<(BodyResolution, IndexedTy)>, PackageStoreError> {
        match value_name {
            BodyValueName::Binding(binding) => {
                let ty = self.body.bindings[binding].ty.clone();
                Ok(Some((BodyResolution::Local(binding), ty)))
            }
            BodyValueName::SemanticItems(items) => {
                let mut functions = Vec::new();
                let mut declarations = Vec::new();
                let mut tys = Vec::new();

                for item in items {
                    match item {
                        SemanticItemRef::Function(function) => {
                            push_unique(&mut functions, ResolvedDeclarationRef::from(function));
                        }
                        SemanticItemRef::Const(const_ref) => {
                            push_unique(&mut declarations, ResolvedDeclarationRef::from(const_ref));
                            push_unique(&mut tys, self.semantic_const_ty(const_ref)?);
                        }
                        SemanticItemRef::Static(static_ref) => {
                            push_unique(
                                &mut declarations,
                                ResolvedDeclarationRef::from(static_ref),
                            );
                            push_unique(&mut tys, self.semantic_static_ty(static_ref)?);
                        }
                        SemanticItemRef::TypeDef(_)
                        | SemanticItemRef::Trait(_)
                        | SemanticItemRef::Impl(_)
                        | SemanticItemRef::TypeAlias(_) => {}
                    }
                }

                if !declarations.is_empty() {
                    return Ok(Some((
                        BodyResolution::Declaration(declarations),
                        unique_ty_or_unknown(tys),
                    )));
                }
                if !functions.is_empty() {
                    return Ok(Some((
                        BodyResolution::Function(functions),
                        IndexedTy::Unknown,
                    )));
                }

                Ok(None)
            }
        }
    }

    fn semantic_const_ty(&self, const_ref: ConstRef) -> Result<IndexedTy, PackageStoreError> {
        let item_query = self.item_query();
        let Some(const_data) = item_query.const_data(const_ref)? else {
            return Ok(IndexedTy::Unknown);
        };
        let Some(ty) = const_data.signature.ty() else {
            return Ok(IndexedTy::Unknown);
        };

        let context = item_query
            .type_path_context_for_owner(const_ref.origin, const_data.owner)?
            .unwrap_or_else(|| rg_semantic_ir::TypePathContext::module(self.body.owner_module));
        if context.module.origin == DefMapRef::Body(self.body_ref) {
            self.type_path_resolver()
                .ty_from_type_ref_in_module_with_subst(ty, context.module, &IndexedTypeSubst::new())
        } else {
            self.type_path_resolver()
                .ty_from_type_ref_in_context_with_subst(ty, context, &IndexedTypeSubst::new())
        }
    }

    fn semantic_static_ty(&self, static_ref: StaticRef) -> Result<IndexedTy, PackageStoreError> {
        let item_query = self.item_query();
        let Some(static_data) = item_query.static_data(static_ref)? else {
            return Ok(IndexedTy::Unknown);
        };
        let Some(ty) = &static_data.ty else {
            return Ok(IndexedTy::Unknown);
        };

        self.type_path_resolver()
            .ty_from_type_ref_in_module_with_subst(ty, static_data.owner, &IndexedTypeSubst::new())
    }

    fn resolve_associated_path(
        &self,
        scope: ScopeId,
        prefix: &Path,
        last_segment: &str,
    ) -> Result<Option<(BodyResolution, IndexedTy)>, PackageStoreError> {
        // Associated value paths are resolved as "type prefix + value member". This keeps
        // `Action::Start` distinct from a module path while also handling `Widget::new` through
        // the same type-substitution rules used by method calls.
        let prefix_resolution = self.type_path_resolver().resolve_in_scope(scope, prefix)?;
        let prefix_ty = self.type_path_resolution_to_ty(prefix_resolution);

        // First treat the final segment as an enum variant. Variants are not ordinary associated
        // functions in either Semantic IR or Body IR, but value paths use the same syntax for
        // `Action::Start` and `Widget::new`, so they need an explicit pass.
        let mut variants = Vec::new();
        let mut variant_tys = Vec::new();
        for nominal_ty in prefix_ty.as_nominals() {
            if !matches!(nominal_ty.def.id, TypeDefId::Enum(_)) {
                continue;
            }
            let Some(variant_ref) = self
                .item_query()
                .enum_variant_ref_for_type_def(nominal_ty.def, last_segment)?
            else {
                continue;
            };
            push_unique(&mut variants, variant_ref);
            push_unique(
                &mut variant_tys,
                IndexedTyRepr::nominal(vec![nominal_ty.clone()]),
            );
        }

        if !variants.is_empty() {
            let ty = unique_ty_or_unknown(variant_tys);
            return Ok(Some((
                BodyResolution::EnumVariant(
                    variants
                        .into_iter()
                        .map(ResolvedDeclarationRef::from)
                        .collect(),
                ),
                ty,
            )));
        }

        for nominal_ty in prefix_ty.as_nominals() {
            if nominal_ty.def.origin != DefMapRef::Body(self.body_ref) {
                continue;
            }
            if let Some((const_ref, ty)) =
                self.semantic_associated_value_item_for_type(nominal_ty, last_segment)?
            {
                return Ok(Some((
                    BodyResolution::Declaration(vec![const_ref.into()]),
                    ty,
                )));
            }
        }

        // Inherent associated functions are exact candidates. Trait-associated functions are kept
        // deliberately optimistic, following the same "prefer useful candidates over false
        // negatives" policy as dot completion.
        let mut functions = Vec::new();
        let item_query = self.item_query();
        for nominal_ty in prefix_ty.as_nominals() {
            for function_ref in self.semantic_associated_functions_for_type(nominal_ty)? {
                let Some(function_data) = item_query.function_data(function_ref)? else {
                    continue;
                };
                if function_data.name == last_segment && !function_data.has_self_receiver() {
                    push_unique(&mut functions, function_ref);
                }
            }
        }

        Ok((!functions.is_empty()).then_some((
            BodyResolution::Function(
                functions
                    .into_iter()
                    .map(ResolvedDeclarationRef::from)
                    .collect(),
            ),
            IndexedTy::Unknown,
        )))
    }

    fn semantic_associated_functions_for_type(
        &self,
        ty: &IndexedNominalTy,
    ) -> Result<Vec<FunctionRef>, PackageStoreError> {
        let mut functions = Vec::new();
        if ty.def.origin == DefMapRef::Body(self.body_ref) {
            let item_query = self.item_query();
            for function in item_query.inherent_functions_for_type(ty.def)? {
                let Some(function_data) = item_query.function_data(function)? else {
                    continue;
                };
                if self.item_store_function_applies_to_receiver(
                    function,
                    function_data.owner,
                    ty,
                )? {
                    functions.push(function);
                }
            }
            return Ok(functions);
        }

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

    fn semantic_associated_value_item_for_type(
        &self,
        ty: &IndexedNominalTy,
        name: &str,
    ) -> Result<Option<(ConstRef, IndexedTy)>, PackageStoreError> {
        let item_query = self.item_query();
        for impl_ref in item_query.inherent_impls_for_type(ty.def)? {
            let Some(impl_data) = item_query.impl_data(impl_ref)? else {
                continue;
            };
            if !self
                .semantic_impl_matcher()
                .impl_applies_to_receiver(impl_ref, impl_data, ty)?
            {
                continue;
            }

            for item in &impl_data.items {
                let AssocItemId::Const(id) = item else {
                    continue;
                };
                let const_ref = ConstRef {
                    origin: impl_ref.origin,
                    id: *id,
                };
                let Some(const_data) = item_query.const_data(const_ref)? else {
                    continue;
                };
                if const_data.name == name {
                    return Ok(Some((
                        const_ref,
                        self.semantic_const_ty_for_receiver(const_ref, const_data.owner, ty)?,
                    )));
                }
            }
        }

        Ok(None)
    }

    fn semantic_const_ty_for_receiver(
        &self,
        const_ref: ConstRef,
        owner: ItemOwner,
        receiver_ty: &IndexedNominalTy,
    ) -> Result<IndexedTy, PackageStoreError> {
        let item_query = self.item_query();
        let Some(const_data) = item_query.const_data(const_ref)? else {
            return Ok(IndexedTy::Unknown);
        };
        let Some(ty) = const_data.signature.ty() else {
            return Ok(IndexedTy::Unknown);
        };

        if type_ref_is_self(ty) {
            return Ok(IndexedTyRepr::nominal(vec![receiver_ty.clone()]));
        }

        let mut subst = self.semantic_type_subst(receiver_ty)?;
        if let ItemOwner::Impl(impl_id) = owner {
            let impl_ref = ImplRef {
                origin: const_ref.origin,
                id: impl_id,
            };
            if let Some(impl_data) = item_query.impl_data(impl_ref)? {
                subst.extend(
                    self.semantic_impl_matcher()
                        .impl_self_subst_for_impl(impl_data, receiver_ty),
                );
            }
        }

        let context = self
            .item_query()
            .type_path_context_for_owner(const_ref.origin, owner)?
            .unwrap_or_else(|| rg_semantic_ir::TypePathContext::module(self.body.owner_module));
        if context.module.origin == DefMapRef::Body(self.body_ref) {
            self.type_path_resolver()
                .ty_from_type_ref_in_module_with_subst(ty, context.module, &subst)
        } else {
            self.type_path_resolver()
                .ty_from_type_ref_in_context_with_subst(ty, context, &subst)
        }
    }

    fn item_store_function_applies_to_receiver(
        &self,
        function_ref: FunctionRef,
        owner: ItemOwner,
        receiver_ty: &IndexedNominalTy,
    ) -> Result<bool, PackageStoreError> {
        let ItemOwner::Impl(impl_id) = owner else {
            return Ok(true);
        };
        let impl_ref = ImplRef {
            origin: function_ref.origin,
            id: impl_id,
        };
        let item_query = self.item_query();
        let Some(impl_data) = item_query.impl_data(impl_ref)? else {
            return Ok(false);
        };
        if impl_data.trait_ref.is_some() {
            return Ok(false);
        }

        self.semantic_impl_matcher()
            .impl_applies_to_receiver(impl_ref, impl_data, receiver_ty)
    }

    fn semantic_type_subst(
        &self,
        ty: &IndexedNominalTy,
    ) -> Result<IndexedTypeSubst, PackageStoreError> {
        Ok(self
            .item_query()
            .generic_params_for_type_def(ty.def)?
            .map(|generics| subst_from_generics(generics, &ty.args))
            .unwrap_or_else(IndexedTypeSubst::new))
    }

    fn type_path_resolution_to_ty(&self, resolution: BodyTypePathResolution) -> IndexedTy {
        match resolution {
            BodyTypePathResolution::SelfType(types) => {
                IndexedTyRepr::self_ty(types.into_iter().map(IndexedNominalTy::bare).collect())
            }
            BodyTypePathResolution::TypeDefs(types) => {
                IndexedTyRepr::nominal(types.into_iter().map(IndexedNominalTy::bare).collect())
            }
            BodyTypePathResolution::Primitive(primitive) => IndexedTy::Primitive(primitive),
            BodyTypePathResolution::TypeAliases(_)
            | BodyTypePathResolution::Traits(_)
            | BodyTypePathResolution::Unknown => IndexedTy::Unknown,
        }
    }

    fn nominal_ty_from_defs(&self, defs: &[DefId]) -> Result<IndexedTy, PackageStoreError> {
        let mut type_defs = Vec::new();
        for def in defs {
            let DefId::Local(local_def) = def else {
                continue;
            };
            let Some(SemanticItemRef::TypeDef(type_def)) =
                self.item_query().semantic_item_for_local_def(*local_def)?
            else {
                continue;
            };
            push_unique(&mut type_defs, type_def);
        }

        Ok(if type_defs.is_empty() {
            IndexedTy::Unknown
        } else {
            IndexedTyRepr::nominal(type_defs.into_iter().map(IndexedNominalTy::bare).collect())
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

fn unique_ty_or_unknown(mut tys: Vec<IndexedTy>) -> IndexedTy {
    if tys.len() == 1 {
        tys.pop().expect("one type should exist")
    } else {
        IndexedTy::Unknown
    }
}
