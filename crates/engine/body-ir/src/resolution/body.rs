//! Main body-resolution pass.
//!
//! This module walks lowered bodies and fills resolution/type slots on bindings and expressions.
//! Specialized helpers live in sibling modules so this file can read like the pass itself.

use rg_ir_model::{
    AssocItemId, BindingId, BodyRef, ConstRef, DefId, DefMapRef, ExprId, FunctionRef, ImplRef,
    ItemOwner, ModuleId, ModuleRef, ScopeId, SemanticItemRef, StaticRef, TypeDefId,
    TypePathResolution, identity::DeclarationRef, items::GenericParams,
};
use rg_ir_storage::{
    DefMapQuery, DefMapSource, ItemLookupIndex, ItemStoreQuery, ItemStoreSource,
    NameResolutionFilter, Path, PathSegment, ResolvePathResult, TargetItemQuery, TypePathContext,
};
use rg_item_tree::{FieldKey, GenericArg as ItemGenericArg};
use rg_package_store::PackageStoreError;
use rg_ty::{
    Autoderef, AutoderefMode, ImplMatcher, ItemPathQuery, NominalTy, PrimitiveTy, Ty, TypeSubst,
};

use crate::{
    ir::body::BodyData,
    ir::expr::{ExprBinaryOp, ExprKind, ExprUnaryOp, ExprWrapperKind},
    ir::resolved::BodyResolution,
    ir::stmt::{BindingKind, BodySelfParamKind},
};

use super::{
    BodyLocalItemQuery, BodyQuerySource, BodyReceiverFunctionQuery, TypeRefUseSite,
    normalize::TyNormalizer, pat::PatternTypePropagator, pat_binding::PatternBindingMaterializer,
    push_unique, type_path::BodyTypePathResolver,
};

pub(crate) struct BodyResolver<'query, 'body, D, I> {
    def_maps: &'query D,
    item_stores: &'query I,
    semantic_index: &'query ItemLookupIndex,
    body_ref: BodyRef,
    body: &'body mut BodyData,
}

impl<'query, 'body, D, I> BodyResolver<'query, 'body, D, I>
where
    for<'source> &'source D: DefMapSource<Error = PackageStoreError>,
    for<'source> &'source I: ItemStoreSource<'source, Error = PackageStoreError>,
{
    pub(crate) fn new(
        def_maps: &'query D,
        item_stores: &'query I,
        semantic_index: &'query ItemLookupIndex,
        body_ref: BodyRef,
        body: &'body mut BodyData,
    ) -> Self {
        Self {
            def_maps,
            item_stores,
            semantic_index,
            body_ref,
            body,
        }
    }

    fn type_path_resolver<'source>(
        &'source self,
    ) -> BodyTypePathResolver<'source, &'source D, &'source I> {
        BodyTypePathResolver::new(self.query_source())
    }

    fn query_source<'source>(&'source self) -> BodyQuerySource<'source, &'source D, &'source I> {
        BodyQuerySource::new(self.def_maps, self.item_stores, self.body_ref, self.body)
    }

    fn autoderef(&self) -> Autoderef<'_, BodyQuerySource<'_, &D, &I>, BodyQuerySource<'_, &D, &I>> {
        let source = self.query_source();
        let item_paths = ItemPathQuery::new(source, source);
        let target_items = TargetItemQuery::new(source, source, self.body_ref.target);
        Autoderef::with_index(item_paths, target_items, self.semantic_index)
    }

    fn impl_matcher(
        &self,
    ) -> ImplMatcher<'_, BodyQuerySource<'_, &D, &I>, BodyQuerySource<'_, &D, &I>> {
        let source = self.query_source();
        let item_paths = ItemPathQuery::new(source, source);
        let target_items = TargetItemQuery::new(source, source, self.body_ref.target);
        ImplMatcher::new(item_paths, target_items)
    }

    fn item_query(&self) -> ItemStoreQuery<'_, BodyQuerySource<'_, &D, &I>> {
        ItemStoreQuery::new(self.query_source())
    }

    fn receiver_functions<'source>(
        &'source self,
    ) -> BodyReceiverFunctionQuery<'source, &'source D, &'source I> {
        BodyReceiverFunctionQuery::new(self.query_source(), Some(self.semantic_index))
    }

    pub(crate) fn resolve(&mut self) -> Result<(), PackageStoreError> {
        self.materialize_pattern_bindings()?;
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
            let binding_updates = PatternTypePropagator::new(self.query_source()).propagate()?;
            changed |= self.apply_binding_type_updates(binding_updates);

            if !changed {
                break;
            }
        }

        Ok(())
    }

    fn materialize_pattern_bindings(&mut self) -> Result<(), PackageStoreError> {
        PatternBindingMaterializer::new(
            self.def_maps,
            self.item_stores,
            self.semantic_index,
            self.body_ref,
            self.body,
        )
        .materialize()
    }

    fn resolve_bindings(&mut self) -> Result<(), PackageStoreError> {
        for binding_idx in 0..self.body.bindings.len() {
            let binding = BindingId(binding_idx);
            let ty = self.binding_ty(binding)?;
            self.body.bindings[binding].ty = ty;
        }
        Ok(())
    }

    fn apply_binding_type_updates(&mut self, updates: Vec<(BindingId, Ty)>) -> bool {
        let mut changed = false;
        for (binding, ty) in updates {
            if matches!(ty, Ty::Unknown) {
                continue;
            }

            let Some(binding_data) = self.body.bindings.get_mut(binding) else {
                continue;
            };
            if !matches!(binding_data.ty, Ty::Unknown) {
                continue;
            }

            binding_data.ty = ty;
            changed = true;
        }

        changed
    }

    fn binding_ty(&self, binding: BindingId) -> Result<Ty, PackageStoreError> {
        let binding_data = &self.body.bindings[binding];
        if let Some(annotation) = &binding_data.annotation {
            return self
                .type_path_resolver()
                .type_ref(TypeRefUseSite::Scope(binding_data.scope))
                .resolve(annotation);
        }

        if let BindingKind::SelfParam(kind) = binding_data.kind
            && binding_data.name.as_deref() == Some("self")
            && let Some(function) = self.body.function_owner()
        {
            let self_tys = self
                .type_path_resolver()
                .self_nominal_tys_for_function(function)?;
            let ty = Ty::self_ty(self_tys);
            return Ok(match kind {
                BodySelfParamKind::Value => ty,
                BodySelfParamKind::Reference { mutability } => Ty::reference(mutability, ty),
                BodySelfParamKind::Explicit => Ty::Unknown,
            });
        }

        Ok(Ty::Unknown)
    }

    fn resolve_expr(&mut self, expr: ExprId) -> Result<bool, PackageStoreError> {
        let old_resolution = self.body.exprs[expr].resolution.clone();
        let old_ty = self.body.exprs[expr].ty.clone();
        let kind = self.body.exprs[expr].kind.clone();

        match kind {
            ExprKind::Path { path } => {
                let (resolution, ty) = match path.as_def_map_path() {
                    Some(path) => self.resolve_path_expr(expr, &path)?,
                    None => (BodyResolution::Unknown, Ty::Unknown),
                };
                let data = &mut self.body.exprs[expr];
                data.resolution = resolution;
                data.ty = ty;
            }
            ExprKind::Call { callee, .. } => {
                self.body.exprs[expr].ty = self.call_ty(callee)?;
            }
            ExprKind::Tuple { fields } => {
                self.body.exprs[expr].ty = self.tuple_expr_ty(&fields);
            }
            ExprKind::Array { elements } => {
                self.body.exprs[expr].ty = self.array_expr_ty(&elements);
            }
            ExprKind::RepeatArray {
                initializer,
                len_text,
                ..
            } => {
                self.body.exprs[expr].ty =
                    self.repeat_array_expr_ty(initializer, len_text.as_deref());
            }
            ExprKind::Index { base, .. } => {
                self.body.exprs[expr].ty = self.index_expr_ty(base);
            }
            ExprKind::Cast { ty: Some(ty), .. } => {
                self.body.exprs[expr].ty = self
                    .type_path_resolver()
                    .type_ref(TypeRefUseSite::Scope(self.body.exprs[expr].scope))
                    .resolve(&ty)?;
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
                    Ty::Unknown
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
                            Ty::Unknown
                        }
                    }
                    None => Ty::Unit,
                };
            }
            ExprKind::Block { tail, .. } => {
                self.body.exprs[expr].ty = tail
                    .map(|tail| self.body.exprs[tail].ty.clone())
                    .unwrap_or(Ty::Unit);
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
                    None => (BodyResolution::Unknown, Ty::Unknown),
                };
                let data = &mut self.body.exprs[expr];
                data.resolution = resolution;
                data.ty = ty;
            }
            ExprKind::MethodCall {
                receiver,
                method_name,
                generic_args,
                ..
            } => {
                let (resolution, ty) = self.resolve_method_call_expr(
                    receiver,
                    &method_name,
                    &generic_args,
                    self.body.exprs[expr].scope,
                )?;
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
            ExprKind::Unary {
                op: Some(op),
                expr: Some(inner),
            } => {
                self.body.exprs[expr].ty = self.unary_ty(op, inner);
            }
            ExprKind::Binary {
                lhs: Some(lhs),
                rhs: Some(rhs),
                op: Some(op),
            } => {
                self.body.exprs[expr].ty = self.binary_ty(op, lhs, rhs);
            }
            ExprKind::Literal { kind } => {
                self.body.exprs[expr].ty = kind.ty();
            }
            ExprKind::While { .. } | ExprKind::For { .. } => {
                self.body.exprs[expr].ty = Ty::Unit;
            }
            ExprKind::Assign { .. } => {
                self.body.exprs[expr].ty = Ty::Unit;
            }
            ExprKind::Break { .. } | ExprKind::Continue { .. } => {
                self.body.exprs[expr].ty = Ty::Never;
            }
            ExprKind::Yeet { .. } | ExprKind::Become { .. } => {
                self.body.exprs[expr].ty = Ty::Never;
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

        Ok(
            self.body.exprs[expr].resolution != old_resolution
                || self.body.exprs[expr].ty != old_ty,
        )
    }

    fn resolve_path_expr(
        &self,
        expr: ExprId,
        path: &Path,
    ) -> Result<(BodyResolution, Ty), PackageStoreError> {
        let scope = self.body.exprs[expr].scope;
        let visible_bindings = self.body.exprs[expr].visible_bindings;
        BodyValuePathResolver::new(self.query_source(), Some(self.semantic_index))
            .resolve_path_expr(scope, path, Some(visible_bindings))
    }

    fn tuple_expr_ty(&self, fields: &[ExprId]) -> Ty {
        Ty::tuple(
            fields
                .iter()
                .map(|field| self.body.exprs[*field].ty.clone())
                .collect(),
        )
    }

    fn array_expr_ty(&self, elements: &[ExprId]) -> Ty {
        if elements.is_empty() {
            return Ty::Unknown;
        }

        let mut element_tys = Vec::new();
        for element in elements {
            let element_ty = self.body.exprs[*element].ty.clone();
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
            self.body.exprs[initializer].ty.clone(),
            len_text.map(str::to_owned),
        )
    }

    fn index_expr_ty(&self, base: Option<ExprId>) -> Ty {
        let Some(base) = base else {
            return Ty::Unknown;
        };

        match &self.body.exprs[base].ty {
            Ty::Array { inner, .. } | Ty::Slice(inner) => inner.as_ref().clone(),
            _ => Ty::Unknown,
        }
    }

    pub(super) fn resolve_nonlocal_path_expr(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<(BodyResolution, Ty), PackageStoreError> {
        BodyValuePathResolver::new(self.query_source(), Some(self.semantic_index))
            .resolve_nonlocal_path_expr(scope, path)
    }

    fn resolve_record_expr_path(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<(BodyResolution, Ty), PackageStoreError> {
        match self.type_path_resolver().resolve_in_scope(scope, path)? {
            TypePathResolution::SelfType(types) => {
                return Ok((
                    BodyResolution::Unknown,
                    Ty::self_ty(types.into_iter().map(NominalTy::bare).collect()),
                ));
            }
            TypePathResolution::TypeDefs(types) => {
                let types = types
                    .into_iter()
                    .filter(|ty| ty.origin == DefMapRef::Body(self.body_ref))
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

        let item_query = self.item_query();
        let mut current_depth = None;
        let mut fields = Vec::new();
        let mut field_tys = Vec::new();

        for candidate in self
            .autoderef()
            .candidates(AutoderefMode::FieldLookup, &self.body.exprs[base].ty)
        {
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
        call_scope: ScopeId,
    ) -> Result<(BodyResolution, Ty), PackageStoreError> {
        let Some(receiver) = receiver else {
            return Ok((BodyResolution::Unknown, Ty::Unknown));
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
        let inner_data = &self.body.exprs[inner];
        let ty = TyNormalizer::new(self.query_source()).ty_for_wrapper(kind, inner_data.ty.clone());
        let resolution = if matches!(kind, ExprWrapperKind::Paren) {
            inner_data.resolution.clone()
        } else {
            BodyResolution::Unknown
        };

        (resolution, ty)
    }

    fn explicit_deref_ty(&self, inner: ExprId) -> Result<Ty, PackageStoreError> {
        let mut candidates = Vec::new();
        for candidate in self
            .autoderef()
            .candidates(AutoderefMode::ExplicitDeref, &self.body.exprs[inner].ty)
        {
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

    fn unary_ty(&self, op: ExprUnaryOp, inner: ExprId) -> Ty {
        let ty = &self.body.exprs[inner].ty;
        match op {
            ExprUnaryOp::Not => match ty {
                Ty::Primitive(primitive) if primitive.is_bool() => Ty::Primitive(*primitive),
                _ => Ty::Unknown,
            },
            ExprUnaryOp::Neg => match ty {
                Ty::Primitive(primitive) if primitive.is_signed_numeric() => {
                    Ty::Primitive(*primitive)
                }
                _ => Ty::Unknown,
            },
            ExprUnaryOp::Deref => Ty::Unknown,
        }
    }

    fn binary_ty(&self, op: ExprBinaryOp, lhs: ExprId, rhs: ExprId) -> Ty {
        let lhs_ty = &self.body.exprs[lhs].ty;
        let rhs_ty = &self.body.exprs[rhs].ty;

        if op.is_logical() || op.is_comparison() {
            return Ty::Primitive(PrimitiveTy::Bool);
        }

        match op {
            ExprBinaryOp::Add
            | ExprBinaryOp::Sub
            | ExprBinaryOp::Mul
            | ExprBinaryOp::Div
            | ExprBinaryOp::Rem => {
                self.symmetric_primitive_op_ty(lhs_ty, rhs_ty, |ty| ty.is_numeric())
            }
            ExprBinaryOp::BitAnd | ExprBinaryOp::BitOr | ExprBinaryOp::BitXor => self
                .symmetric_primitive_op_ty(lhs_ty, rhs_ty, |ty| ty.is_integral() || ty.is_bool()),
            ExprBinaryOp::Shl | ExprBinaryOp::Shr => self.shift_op_ty(lhs_ty, rhs_ty),
            ExprBinaryOp::LogicOr
            | ExprBinaryOp::LogicAnd
            | ExprBinaryOp::Eq
            | ExprBinaryOp::NotEq
            | ExprBinaryOp::Less
            | ExprBinaryOp::LessEq
            | ExprBinaryOp::Greater
            | ExprBinaryOp::GreaterEq => Ty::Primitive(PrimitiveTy::Bool),
        }
    }

    fn symmetric_primitive_op_ty(
        &self,
        lhs_ty: &Ty,
        rhs_ty: &Ty,
        accepts: impl Fn(PrimitiveTy) -> bool,
    ) -> Ty {
        match (lhs_ty, rhs_ty) {
            (Ty::Primitive(lhs), Ty::Primitive(rhs)) if lhs == rhs && accepts(*lhs) => {
                Ty::Primitive(*lhs)
            }
            (Ty::Primitive(lhs), Ty::Unknown) if accepts(*lhs) => Ty::Primitive(*lhs),
            (Ty::Unknown, Ty::Primitive(rhs)) if accepts(*rhs) => Ty::Primitive(*rhs),
            _ => Ty::Unknown,
        }
    }

    fn shift_op_ty(&self, lhs_ty: &Ty, rhs_ty: &Ty) -> Ty {
        match (lhs_ty, rhs_ty) {
            (Ty::Primitive(lhs), Ty::Primitive(rhs)) if lhs.is_integral() && rhs.is_integral() => {
                Ty::Primitive(*lhs)
            }
            (Ty::Primitive(lhs), Ty::Unknown) if lhs.is_integral() => Ty::Primitive(*lhs),
            _ => Ty::Unknown,
        }
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
        let item_query = self.item_query();
        let Some(impl_data) = item_query.impl_data(ImplRef {
            origin: function_ref.origin,
            id: impl_id,
        })?
        else {
            return Ok(TypeSubst::new());
        };

        Ok(self
            .impl_matcher()
            .impl_self_subst_for_impl(impl_data, receiver_ty))
    }

    fn semantic_type_subst(&self, ty: &NominalTy) -> Result<TypeSubst, PackageStoreError> {
        Ok(self
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
        call_scope: Option<ScopeId>,
    ) -> Result<Ty, PackageStoreError> {
        let Some(function_data) = self.item_query().function_data(function_ref)? else {
            return Ok(Ty::Unknown);
        };
        let mut subst = receiver_ty
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
        if let Some(call_scope) = call_scope {
            subst.extend(self.explicit_function_subst(
                function_data.signature.generics(),
                explicit_args,
                call_scope,
            )?);
        }
        self.semantic_function_return_ty_with_subst(
            function_ref,
            receiver_ty.cloned().map(|ty| Ty::nominal(vec![ty])),
            subst,
        )
    }

    fn semantic_function_return_ty_with_subst(
        &self,
        function_ref: FunctionRef,
        self_ty: Option<Ty>,
        subst: TypeSubst,
    ) -> Result<Ty, PackageStoreError> {
        self.semantic_function_return_ty_with_subst_and_call_args(
            function_ref,
            self_ty,
            subst,
            &[],
            None,
        )
    }

    fn semantic_function_return_ty_with_subst_and_call_args(
        &self,
        function_ref: FunctionRef,
        self_ty: Option<Ty>,
        mut subst: TypeSubst,
        explicit_args: &[ItemGenericArg],
        call_scope: Option<ScopeId>,
    ) -> Result<Ty, PackageStoreError> {
        let item_query = self.item_query();
        let Some(function_data) = item_query.function_data(function_ref)? else {
            return Ok(Ty::Unknown);
        };
        if let Some(call_scope) = call_scope {
            subst.extend(self.explicit_function_subst(
                function_data.signature.generics(),
                explicit_args,
                call_scope,
            )?);
        }
        let Some(ret_ty) = function_data.signature.ret_ty() else {
            return Ok(Ty::Unit);
        };

        if ret_ty.is_self_type() {
            return Ok(match self_ty {
                Some(self_ty) => self_ty,
                None => Ty::self_ty(
                    self.type_path_resolver()
                        .self_nominal_tys_for_function(function_ref)?,
                ),
            });
        }

        self.type_path_resolver()
            .type_ref(TypeRefUseSite::Function(function_ref))
            .with_subst(&subst)
            .resolve(ret_ty)
    }

    fn call_ty(&self, callee: Option<ExprId>) -> Result<Ty, PackageStoreError> {
        let Some(callee) = callee else {
            return Ok(Ty::Unknown);
        };
        let callee_data = &self.body.exprs[callee];

        if matches!(callee_data.ty, Ty::Nominal(_) | Ty::SelfTy(_)) {
            return Ok(callee_data.ty.clone());
        }

        // Ordinary calls use explicit return types only. Generic function inference remains
        // outside the current intentionally-small Body IR model.
        let mut return_tys = Vec::new();
        match &callee_data.resolution {
            BodyResolution::Declarations(declarations) => {
                for declaration in declarations {
                    self.push_return_ty_for_declaration(
                        *declaration,
                        &mut return_tys,
                        self.explicit_callee_generic_args(callee_data),
                        callee_data.scope,
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
        callee_data: &'expr crate::ir::expr::ExprData,
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
        let type_resolver = self.type_path_resolver();
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
            match self.item_query().semantic_item_for_local_def(local_def)? {
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
pub(crate) struct BodyValuePathResolver<'query, D, I> {
    source: BodyQuerySource<'query, D, I>,
    semantic_index: Option<&'query ItemLookupIndex>,
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

impl<'query, D, I> BodyValuePathResolver<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(
        source: BodyQuerySource<'query, D, I>,
        semantic_index: Option<&'query ItemLookupIndex>,
    ) -> Self {
        Self {
            source,
            semantic_index,
        }
    }

    fn type_path_resolver(&self) -> BodyTypePathResolver<'query, D, I> {
        BodyTypePathResolver::new(self.source)
    }

    fn impl_matcher(
        &self,
    ) -> ImplMatcher<'query, BodyQuerySource<'query, D, I>, BodyQuerySource<'query, D, I>> {
        let source = self.source;
        let item_paths = ItemPathQuery::new(source, source);
        let target_items = TargetItemQuery::new(source, source, self.source.body_ref().target);
        ImplMatcher::new(item_paths, target_items)
    }

    fn item_query(&self) -> ItemStoreQuery<'query, BodyQuerySource<'query, D, I>> {
        ItemStoreQuery::new(self.source)
    }

    fn body_local_items(&self) -> BodyLocalItemQuery<'query, D, I> {
        BodyLocalItemQuery::new(self.source)
    }

    fn receiver_functions(&self) -> BodyReceiverFunctionQuery<'query, D, I> {
        BodyReceiverFunctionQuery::new(self.source, self.semantic_index)
    }

    fn def_map_query(&self) -> DefMapQuery<BodyQuerySource<'query, D, I>> {
        DefMapQuery::new(self.source)
    }

    pub(crate) fn resolve_nonlocal_path_expr(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<(BodyResolution, Ty), PackageStoreError> {
        self.resolve_path_expr(scope, path, None)
    }

    pub(super) fn resolve_path_expr(
        &self,
        scope: ScopeId,
        path: &Path,
        visible_bindings: Option<usize>,
    ) -> Result<(BodyResolution, Ty), PackageStoreError> {
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
            TypePathResolution::SelfType(types) => {
                return Ok((
                    BodyResolution::Unknown,
                    Ty::self_ty(types.into_iter().map(NominalTy::bare).collect()),
                ));
            }
            TypePathResolution::TypeDefs(types) => {
                let mut constructors = Vec::new();
                for type_def in types
                    .into_iter()
                    .filter(|ty| ty.origin == DefMapRef::Body(self.source.body_ref()))
                {
                    if self.item_query().type_def_has_value_constructor(type_def)? {
                        push_unique(&mut constructors, type_def);
                    }
                }

                if !constructors.is_empty() {
                    return Ok((
                        BodyResolution::Declarations(
                            constructors
                                .iter()
                                .copied()
                                .map(DeclarationRef::from)
                                .collect(),
                        ),
                        Ty::nominal(constructors.into_iter().map(NominalTy::bare).collect()),
                    ));
                }
            }
            TypePathResolution::TypeAliases(_)
            | TypePathResolution::Traits(_)
            | TypePathResolution::Unknown => {}
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

        let result = self.resolve_path_from_owner_modules(path)?;
        if result.resolved.is_empty() {
            return Ok((BodyResolution::Unknown, Ty::Unknown));
        }
        let ty = self.nominal_ty_from_defs(&result.resolved)?;
        Ok((
            BodyResolution::Declarations(
                result
                    .resolved
                    .into_iter()
                    .map(DeclarationRef::from)
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
    ) -> Result<Option<(BodyResolution, Ty)>, PackageStoreError> {
        // Value lookup is scope-ordered: an inner const/function shadows an outer binding just as
        // surely as an inner binding shadows an outer item.
        let from = ModuleRef {
            origin: DefMapRef::Body(self.source.body_ref()),
            module: ModuleId(start_scope.0),
        };
        let mut scope = Some(start_scope);
        while let Some(scope_id) = scope {
            let Some(scope_data) = self.source.body().scope(scope_id) else {
                return Ok(None);
            };

            if let Some(visible_bindings) = visible_bindings {
                for binding in scope_data.bindings.iter().rev() {
                    if binding.0 >= visible_bindings {
                        continue;
                    }

                    let Some(binding_data) = self.source.body().binding(*binding) else {
                        continue;
                    };
                    if binding_data.name.as_deref() == Some(name) {
                        return self.value_name_resolution(BodyValueName::Binding(*binding));
                    }
                }
            }

            let module = ModuleRef {
                origin: DefMapRef::Body(self.source.body_ref()),
                module: ModuleId(scope_id.0),
            };
            let defs = self.def_map_query().resolve_lexical_name_in_module(
                from,
                module,
                name,
                NameResolutionFilter::ValuesOnly,
            )?;
            let value_name = BodyValueName::SemanticItems(self.semantic_items_for_defs(defs)?);
            if let Some(resolution) = self.value_name_resolution(value_name)? {
                return Ok(Some(resolution));
            }

            scope = scope_data.parent;
        }

        Ok(None)
    }

    fn resolve_path_from_owner_modules(
        &self,
        path: &Path,
    ) -> Result<ResolvePathResult, PackageStoreError> {
        let owner_module = self.source.body().owner_module();
        let result = self.def_map_query().resolve_path(owner_module, path)?;
        if !result.resolved.is_empty() {
            return Ok(result);
        }

        let fallback_module = self.source.body().fallback_module();
        if fallback_module == owner_module {
            return Ok(result);
        }

        self.def_map_query().resolve_path(fallback_module, path)
    }

    fn resolve_body_value_path_from_def_map(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<Option<(BodyResolution, Ty)>, PackageStoreError> {
        let from = ModuleRef {
            origin: DefMapRef::Body(self.source.body_ref()),
            module: ModuleId(scope.0),
        };
        let defs = self
            .def_map_query()
            .resolve_lexical_path(from, path, NameResolutionFilter::ValuesOnly)?
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
    ) -> Result<Option<(BodyResolution, Ty)>, PackageStoreError> {
        match value_name {
            BodyValueName::Binding(binding) => {
                let ty = self.source.body().bindings[binding].ty.clone();
                Ok(Some((BodyResolution::Binding(binding), ty)))
            }
            BodyValueName::SemanticItems(items) => {
                let mut functions = Vec::new();
                let mut declarations = Vec::new();
                let mut tys = Vec::new();

                for item in items {
                    match item {
                        SemanticItemRef::Function(function) => {
                            push_unique(&mut functions, DeclarationRef::from(function));
                        }
                        SemanticItemRef::Const(const_ref) => {
                            push_unique(&mut declarations, DeclarationRef::from(const_ref));
                            push_unique(&mut tys, self.semantic_const_ty(const_ref)?);
                        }
                        SemanticItemRef::Static(static_ref) => {
                            push_unique(&mut declarations, DeclarationRef::from(static_ref));
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
                        BodyResolution::Declarations(declarations),
                        unique_ty_or_unknown(tys),
                    )));
                }
                if !functions.is_empty() {
                    return Ok(Some((BodyResolution::Declarations(functions), Ty::Unknown)));
                }

                Ok(None)
            }
        }
    }

    fn semantic_const_ty(&self, const_ref: ConstRef) -> Result<Ty, PackageStoreError> {
        let item_query = self.item_query();
        let Some(const_data) = item_query.const_data(const_ref)? else {
            return Ok(Ty::Unknown);
        };
        let Some(ty) = const_data.signature.ty() else {
            return Ok(Ty::Unknown);
        };

        let context = item_query
            .type_path_context_for_owner(const_ref.origin, const_data.owner)?
            .unwrap_or_else(|| TypePathContext::module(self.source.body().owner_module));
        self.type_path_resolver()
            .type_ref(TypeRefUseSite::OwnerContext(context))
            .resolve(ty)
    }

    fn semantic_static_ty(&self, static_ref: StaticRef) -> Result<Ty, PackageStoreError> {
        let item_query = self.item_query();
        let Some(static_data) = item_query.static_data(static_ref)? else {
            return Ok(Ty::Unknown);
        };
        let Some(ty) = &static_data.ty else {
            return Ok(Ty::Unknown);
        };

        self.type_path_resolver()
            .type_ref(TypeRefUseSite::Module(static_data.owner))
            .resolve(ty)
    }

    fn resolve_associated_path(
        &self,
        scope: ScopeId,
        prefix: &Path,
        last_segment: &str,
    ) -> Result<Option<(BodyResolution, Ty)>, PackageStoreError> {
        // Associated value paths are resolved as "type prefix + value member". This keeps
        // `Action::Start` distinct from a module path while also handling `Widget::new` through
        // the same type-substitution rules used by method calls.
        let prefix_resolution = self.type_path_resolver().resolve_in_scope(scope, prefix)?;
        let prefix_ty =
            Ty::from_type_path_resolution(prefix_resolution, Vec::new()).unwrap_or(Ty::Unknown);

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
            push_unique(&mut variant_tys, Ty::nominal(vec![nominal_ty.clone()]));
        }

        if !variants.is_empty() {
            let ty = unique_ty_or_unknown(variant_tys);
            return Ok(Some((
                BodyResolution::Declarations(
                    variants.into_iter().map(DeclarationRef::from).collect(),
                ),
                ty,
            )));
        }

        for nominal_ty in prefix_ty.as_nominals() {
            if let Some((const_ref, ty)) =
                self.semantic_associated_value_item_for_type(nominal_ty, last_segment)?
            {
                return Ok(Some((
                    BodyResolution::Declarations(vec![const_ref.into()]),
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
            for function_ref in self
                .receiver_functions()
                .function_refs_for_receiver(nominal_ty, Some(last_segment))?
            {
                let Some(function_data) = item_query.function_data(function_ref)? else {
                    continue;
                };
                if function_data.name == last_segment && !function_data.has_self_receiver() {
                    push_unique(&mut functions, function_ref);
                }
            }
        }

        Ok((!functions.is_empty()).then_some((
            BodyResolution::Declarations(functions.into_iter().map(DeclarationRef::from).collect()),
            Ty::Unknown,
        )))
    }

    fn semantic_associated_value_item_for_type(
        &self,
        ty: &NominalTy,
        name: &str,
    ) -> Result<Option<(ConstRef, Ty)>, PackageStoreError> {
        if let Some(item) = self.associated_value_item_for_impls(
            self.body_local_items().inherent_impls_for_type(ty.def)?,
            ty,
            name,
        )? {
            return Ok(Some(item));
        }

        if ty.def.origin == DefMapRef::Body(self.source.body_ref()) {
            return Ok(None);
        }

        let source = self.source;
        let target_items = TargetItemQuery::new(source, source, self.source.body_ref().target);
        self.associated_value_item_for_impls(
            target_items.inherent_impls_for_type(ty.def)?,
            ty,
            name,
        )
    }

    fn associated_value_item_for_impls(
        &self,
        impls: Vec<ImplRef>,
        ty: &NominalTy,
        name: &str,
    ) -> Result<Option<(ConstRef, Ty)>, PackageStoreError> {
        let item_query = self.item_query();
        for impl_ref in impls {
            let Some(impl_data) = item_query.impl_data(impl_ref)? else {
                continue;
            };
            if !self
                .impl_matcher()
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
        receiver_ty: &NominalTy,
    ) -> Result<Ty, PackageStoreError> {
        let item_query = self.item_query();
        let Some(const_data) = item_query.const_data(const_ref)? else {
            return Ok(Ty::Unknown);
        };
        let Some(ty) = const_data.signature.ty() else {
            return Ok(Ty::Unknown);
        };

        if ty.is_self_type() {
            return Ok(Ty::nominal(vec![receiver_ty.clone()]));
        }

        let mut subst = self.semantic_type_subst(receiver_ty)?;
        if let ItemOwner::Impl(impl_id) = owner {
            let impl_ref = ImplRef {
                origin: const_ref.origin,
                id: impl_id,
            };
            if let Some(impl_data) = item_query.impl_data(impl_ref)? {
                subst.extend(
                    self.impl_matcher()
                        .impl_self_subst_for_impl(impl_data, receiver_ty),
                );
            }
        }

        let context = self
            .item_query()
            .type_path_context_for_owner(const_ref.origin, owner)?
            .unwrap_or_else(|| TypePathContext::module(self.source.body().owner_module));
        self.type_path_resolver()
            .type_ref(TypeRefUseSite::OwnerContext(context))
            .with_subst(&subst)
            .resolve(ty)
    }

    fn semantic_type_subst(&self, ty: &NominalTy) -> Result<TypeSubst, PackageStoreError> {
        Ok(self
            .item_query()
            .generic_params_for_type_def(ty.def)?
            .map(|generics| TypeSubst::from_generics(generics, &ty.args))
            .unwrap_or_else(TypeSubst::new))
    }

    fn nominal_ty_from_defs(&self, defs: &[DefId]) -> Result<Ty, PackageStoreError> {
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
            Ty::Unknown
        } else {
            Ty::nominal(type_defs.into_iter().map(NominalTy::bare).collect())
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

fn unique_ty_or_unknown(mut tys: Vec<Ty>) -> Ty {
    if tys.len() == 1 {
        tys.pop().expect("one type should exist")
    } else {
        Ty::Unknown
    }
}
