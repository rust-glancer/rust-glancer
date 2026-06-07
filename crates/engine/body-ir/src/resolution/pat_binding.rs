//! Pattern binding candidate materialization.
//!
//! Lowering has to build scopes before value-name resolution is available, so ambiguous identifier
//! patterns temporarily occupy binding slots. This pass decides which slots are real bindings and
//! compacts the body back to the final binding arena used by all later resolver and analysis code.
//!
//! The pass has three phases:
//!
//! 1. Decide which pending binding slots stay active.
//! 2. Use known pattern input types to catch unit variants such as `None`.
//! 3. Rewrite every binding reference from pending slot ids to final binding ids.

use rg_ir_model::{
    BindingId, BodyRef, DefId, DefMapRef, ExprId, ModuleId, ModuleRef, ScopeId, SemanticItemRef,
    TypeDefId, identity::DeclarationRef,
};
use rg_ir_storage::{
    DefMapQuery, DefMapSource, ItemLookupIndex, ItemStoreQuery, ItemStoreSource,
    NameResolutionFilter, Path, PathSegment,
};
use rg_item_tree::{FieldItem, FieldKey, FieldList};
use rg_package_store::PackageStoreError;
use rg_ty::{NominalTy, ReferencePeelingCandidates, Ty, TypeSubst};

use crate::{
    BodyPath,
    ir::body::ResolvedBodyData,
    ir::expr::ExprKind,
    ir::pat::{PatKind, RecordPatField},
    ir::resolved::BodyResolution,
    ir::stmt::{BindingKind, BodySelfParamKind, PendingBindingResolution, StmtKind},
};

use super::{
    BodyQuerySource, BodyTypePathResolver, BodyValuePathResolver, TypeRefUseSite, push_unique,
};

/// Resolves lowered binding candidates into the final body binding arena.
///
/// After this pass, consumers should see ordinary `BindingId`s only. Ambiguous pattern identifiers
/// that resolved as consts/statics/unit variants remain visible through their pattern path, not as
/// fake local bindings.
pub(super) struct PatternBindingMaterializer<'query, 'body, D, I> {
    def_maps: &'query D,
    item_stores: &'query I,
    semantic_index: &'query ItemLookupIndex,
    body_ref: BodyRef,
    body: &'body mut ResolvedBodyData,
}

impl<'query, 'body, D, I> PatternBindingMaterializer<'query, 'body, D, I>
where
    for<'source> &'source D: DefMapSource<Error = PackageStoreError>,
    for<'source> &'source I: ItemStoreSource<'source, Error = PackageStoreError>,
{
    pub(super) fn new(
        def_maps: &'query D,
        item_stores: &'query I,
        semantic_index: &'query ItemLookupIndex,
        body_ref: BodyRef,
        body: &'body mut ResolvedBodyData,
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
        BodyQuerySource::new(self.def_maps, self.item_stores, self.body_ref, &*self.body)
    }

    fn item_query(&self) -> ItemStoreQuery<'_, BodyQuerySource<'_, &D, &I>> {
        ItemStoreQuery::new(self.query_source())
    }

    /// Materializes all pending binding candidates, leaving the body in its final binding shape.
    pub(super) fn materialize(&mut self) -> Result<(), PackageStoreError> {
        if self.body.pending_binding_resolutions.is_empty() {
            return Ok(());
        }

        // `active` is indexed by the original pending binding ids. We keep this temporary view
        // while lookup still needs source-order visibility against the lowered scope lists.
        let pending_count = self.body.bindings().len();
        let mut active = Vec::with_capacity(pending_count);
        for binding_idx in 0..pending_count {
            let binding = BindingId(binding_idx);
            let is_active = match self.body.pending_binding_resolutions[binding] {
                PendingBindingResolution::AlwaysBinding => true,
                PendingBindingResolution::AmbiguousPattern => {
                    !self.ambiguous_pattern_binding_resolves_as_value(binding, &active)?
                }
            };
            active.push(is_active);
        }

        // Value lookup catches constants and statics. Unit variants also need the type of the
        // pattern input, so run that pass after the first active binding set exists.
        let pending_tys = self.pending_binding_tys(&active)?;
        self.deactivate_unit_variant_pattern_bindings(&mut active, &pending_tys)?;
        self.body.compact_bindings(active);
        Ok(())
    }

    /// Returns binding types in the pending-id space used during materialization.
    fn pending_binding_tys(&self, active: &[bool]) -> Result<Vec<Ty>, PackageStoreError> {
        let mut tys = Vec::with_capacity(self.body.bindings().len());
        for binding_idx in 0..self.body.bindings().len() {
            let binding = BindingId(binding_idx);
            if active.get(binding_idx).copied().unwrap_or(false) {
                tys.push(self.binding_ty(binding)?);
            } else {
                tys.push(Ty::Unknown);
            }
        }
        Ok(tys)
    }

    fn deactivate_unit_variant_pattern_bindings(
        &self,
        active: &mut [bool],
        pending_tys: &[Ty],
    ) -> Result<(), PackageStoreError> {
        // A bare `None` cannot be recognized from value lookup alone unless it is written as a
        // full path. The input type of `match value` or `let pat: Ty = ...` gives enough context to
        // interpret single-segment unit variants without returning to capitalization heuristics.
        for statement in self.body.statements().iter() {
            let StmtKind::Let {
                scope,
                pat: Some(pat),
                annotation,
                initializer,
                ..
            } = &statement.kind
            else {
                continue;
            };

            let expected_ty = self.pending_expected_ty_for_let(
                *scope,
                annotation.as_ref(),
                *initializer,
                active,
                pending_tys,
            )?;
            self.deactivate_unit_variant_bindings_in_pat(*pat, &expected_ty, active)?;
        }

        for expr_idx in 0..self.body.exprs().len() {
            let expr = ExprId(expr_idx);
            match &self.body.expr_unchecked(expr).kind {
                ExprKind::Match { scrutinee, arms } => {
                    let expected_ty = scrutinee
                        .map(|scrutinee| self.pending_expr_ty(scrutinee, active, pending_tys))
                        .transpose()?
                        .unwrap_or(Ty::Unknown);
                    for arm in arms {
                        if let Some(pat) = arm.pat {
                            self.deactivate_unit_variant_bindings_in_pat(
                                pat,
                                &expected_ty,
                                active,
                            )?;
                        }
                    }
                }
                ExprKind::Let {
                    scope,
                    pat: Some(pat),
                    initializer,
                    ..
                } => {
                    let expected_ty = self.pending_expected_ty_for_let(
                        *scope,
                        None,
                        *initializer,
                        active,
                        pending_tys,
                    )?;
                    self.deactivate_unit_variant_bindings_in_pat(*pat, &expected_ty, active)?;
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn pending_expected_ty_for_let(
        &self,
        scope: ScopeId,
        annotation: Option<&rg_item_tree::TypeRef>,
        initializer: Option<ExprId>,
        active: &[bool],
        pending_tys: &[Ty],
    ) -> Result<Ty, PackageStoreError> {
        if let Some(annotation) = annotation {
            let ty = self
                .type_path_resolver()
                .type_ref(TypeRefUseSite::Scope(scope))
                .resolve(annotation)?;
            if !matches!(ty, Ty::Unknown) {
                return Ok(ty);
            }
        }

        initializer
            .map(|expr| self.pending_expr_ty(expr, active, pending_tys))
            .transpose()
            .map(|ty| ty.unwrap_or(Ty::Unknown))
    }

    fn pending_expr_ty(
        &self,
        expr: ExprId,
        active: &[bool],
        pending_tys: &[Ty],
    ) -> Result<Ty, PackageStoreError> {
        match &self.body.expr_unchecked(expr).kind {
            ExprKind::Path { path } => {
                let Some(path) = path.as_def_map_path() else {
                    return Ok(Ty::Unknown);
                };
                Ok(self
                    .pending_binding_ty_for_path(
                        self.body.expr_unchecked(expr).scope,
                        self.body.expr_unchecked(expr).visible_bindings,
                        &path,
                        active,
                        pending_tys,
                    )
                    .unwrap_or(Ty::Unknown))
            }
            ExprKind::Wrapper {
                inner: Some(inner), ..
            } => {
                // This is only cheap context recovery for pattern disambiguation. Peeking through
                // wrappers keeps common cases useful without trying to type-check the expression.
                self.pending_expr_ty(*inner, active, pending_tys)
            }
            _ => Ok(self.body.expr_ty_unchecked(expr).clone()),
        }
    }

    fn pending_binding_ty_for_path(
        &self,
        start_scope: ScopeId,
        visible_bindings: usize,
        path: &Path,
        active: &[bool],
        pending_tys: &[Ty],
    ) -> Option<Ty> {
        let name = path.single_name()?;
        let mut scope = Some(start_scope);
        while let Some(scope_id) = scope {
            let scope_data = self.body.scope(scope_id)?;
            for binding in scope_data.bindings.iter().rev() {
                if binding.0 >= visible_bindings {
                    continue;
                }
                if !active.get(binding.0).copied().unwrap_or(false) {
                    continue;
                }
                let Some(binding_data) = self.body.binding(*binding) else {
                    continue;
                };
                if binding_data.name.as_deref() == Some(name) {
                    return pending_tys.get(binding.0).cloned();
                }
            }
            scope = scope_data.parent;
        }

        None
    }

    /// Walks a pattern with its input type and deactivates ambiguous unit-variant bindings.
    fn deactivate_unit_variant_bindings_in_pat(
        &self,
        pat: rg_ir_model::PatId,
        expected_ty: &Ty,
        active: &mut [bool],
    ) -> Result<(), PackageStoreError> {
        if matches!(expected_ty, Ty::Unknown) {
            return Ok(());
        }

        let Some(data) = self.body.pat(pat) else {
            return Ok(());
        };
        match &data.kind {
            PatKind::Binding {
                binding,
                subpat,
                path,
                ..
            } => {
                if let Some(binding) = binding
                    && active.get(binding.0).copied().unwrap_or(false)
                    && matches!(
                        self.body.pending_binding_resolutions[*binding],
                        PendingBindingResolution::AmbiguousPattern
                    )
                    && path
                        .as_ref()
                        .map(|path| self.path_is_unit_variant_pattern(path, expected_ty))
                        .transpose()?
                        .unwrap_or(false)
                {
                    active[binding.0] = false;
                }
                if let Some(subpat) = subpat {
                    self.deactivate_unit_variant_bindings_in_pat(*subpat, expected_ty, active)?;
                }
            }
            PatKind::TupleStruct { path, fields } => {
                self.deactivate_unit_variant_bindings_in_tuple_struct_pat(
                    path.as_ref(),
                    fields,
                    expected_ty,
                    active,
                )?;
            }
            PatKind::Record { path, fields, .. } => {
                self.deactivate_unit_variant_bindings_in_record_pat(
                    path.as_ref(),
                    fields,
                    expected_ty,
                    active,
                )?;
            }
            PatKind::Or { pats } => {
                for pat in pats {
                    self.deactivate_unit_variant_bindings_in_pat(*pat, expected_ty, active)?;
                }
            }
            PatKind::Ref { pat, .. } | PatKind::Box { pat } => {
                self.deactivate_unit_variant_bindings_in_pat(*pat, expected_ty, active)?;
            }
            PatKind::Tuple { .. }
            | PatKind::Slice { .. }
            | PatKind::Path { .. }
            | PatKind::Rest
            | PatKind::Literal { .. }
            | PatKind::Range { .. }
            | PatKind::ConstBlock { .. }
            | PatKind::Wildcard
            | PatKind::Unsupported => {}
        }

        Ok(())
    }

    fn deactivate_unit_variant_bindings_in_tuple_struct_pat(
        &self,
        path: Option<&BodyPath>,
        fields: &[rg_ir_model::PatId],
        expected_ty: &Ty,
        active: &mut [bool],
    ) -> Result<(), PackageStoreError> {
        for (index, field_pat) in fields.iter().enumerate() {
            let field_key = FieldKey::Tuple(index);
            let Some(field_ty) = self.pattern_field_ty(path, expected_ty, &field_key)? else {
                continue;
            };
            self.deactivate_unit_variant_bindings_in_pat(*field_pat, &field_ty, active)?;
        }
        Ok(())
    }

    fn deactivate_unit_variant_bindings_in_record_pat(
        &self,
        path: Option<&BodyPath>,
        fields: &[RecordPatField],
        expected_ty: &Ty,
        active: &mut [bool],
    ) -> Result<(), PackageStoreError> {
        for field in fields {
            let Some(field_ty) = self.pattern_field_ty(path, expected_ty, &field.key)? else {
                continue;
            };
            self.deactivate_unit_variant_bindings_in_pat(field.pat, &field_ty, active)?;
        }
        Ok(())
    }

    fn pattern_field_ty(
        &self,
        path: Option<&BodyPath>,
        expected_ty: &Ty,
        field_key: &FieldKey,
    ) -> Result<Option<Ty>, PackageStoreError> {
        let def_map_path = path.and_then(|path| path.as_def_map_path());
        let variant_name = Self::pattern_path_last_name(def_map_path.as_ref());
        let mut candidates = Vec::new();

        // Pattern fields are checked against the type of the field they destructure. This matters
        // before final binding materialization because `None` in `User { value: None }` only makes
        // sense after we project `User::value` to `Option<_>`.
        for candidate in ReferencePeelingCandidates::new(expected_ty) {
            for nominal_ty in candidate.ty().as_nominals() {
                match nominal_ty.def.id {
                    TypeDefId::Struct(_) | TypeDefId::Union(_) => {
                        if let Some(field_ty) =
                            self.field_ty_for_nominal_type(nominal_ty, field_key)?
                        {
                            push_unique(&mut candidates, field_ty);
                        }
                    }
                    TypeDefId::Enum(_) => {
                        let Some(variant_name) = variant_name else {
                            continue;
                        };
                        if let Some(field_ty) =
                            self.variant_field_ty_for_enum(nominal_ty, variant_name, field_key)?
                        {
                            push_unique(&mut candidates, field_ty);
                        }
                    }
                }
            }
        }

        Ok(match candidates.as_slice() {
            [ty] => Some(ty.clone()),
            [] | [_, ..] => None,
        })
    }

    fn field_ty_for_nominal_type(
        &self,
        ty: &NominalTy,
        field_key: &FieldKey,
    ) -> Result<Option<Ty>, PackageStoreError> {
        let item_query = self.item_query();
        let Some(field_ref) = item_query.field_for_type(ty.def, field_key)? else {
            return Ok(None);
        };
        let Some(field_data) = item_query.field_data(field_ref)? else {
            return Ok(None);
        };

        Ok(Some(
            self.type_path_resolver()
                .type_ref(TypeRefUseSite::Module(field_data.owner_module))
                .with_subst(&self.semantic_type_subst(ty)?)
                .resolve(&field_data.field.ty)?,
        ))
    }

    fn variant_field_ty_for_enum(
        &self,
        enum_ty: &NominalTy,
        variant_name: &str,
        field_key: &FieldKey,
    ) -> Result<Option<Ty>, PackageStoreError> {
        let item_query = self.item_query();
        let Some(variant_ref) =
            item_query.enum_variant_ref_for_type_def(enum_ty.def, variant_name)?
        else {
            return Ok(None);
        };
        let Some(variant_data) = item_query.enum_variant_data(variant_ref)? else {
            return Ok(None);
        };
        let Some(field) = Self::field_item(&variant_data.variant.fields, field_key) else {
            return Ok(None);
        };

        Ok(Some(
            self.type_path_resolver()
                .type_ref(TypeRefUseSite::Module(variant_data.owner_module))
                .with_subst(&self.semantic_type_subst(enum_ty)?)
                .resolve(&field.ty)?,
        ))
    }

    fn path_is_unit_variant_pattern(
        &self,
        path: &BodyPath,
        expected_ty: &Ty,
    ) -> Result<bool, PackageStoreError> {
        let Some(path) = path.as_def_map_path() else {
            return Ok(false);
        };
        let Some(variant_name) = path.single_name() else {
            return Ok(false);
        };

        for candidate in ReferencePeelingCandidates::new(expected_ty) {
            for enum_ty in candidate
                .ty()
                .as_nominals()
                .iter()
                .filter(|ty| matches!(ty.def.id, TypeDefId::Enum(_)))
            {
                let Some(variant_ref) = self
                    .item_query()
                    .enum_variant_ref_for_type_def(enum_ty.def, variant_name)?
                else {
                    continue;
                };
                let Some(variant_data) = self.item_query().enum_variant_data(variant_ref)? else {
                    continue;
                };
                if matches!(variant_data.variant.fields, FieldList::Unit) {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    /// Returns true when an ambiguous identifier pattern should stay a value path.
    fn ambiguous_pattern_binding_resolves_as_value(
        &self,
        binding: BindingId,
        active: &[bool],
    ) -> Result<bool, PackageStoreError> {
        let Some(binding_data) = self.body.binding(binding) else {
            return Ok(false);
        };
        let Some(name) = binding_data.name.as_deref() else {
            return Ok(false);
        };

        // Local bindings shadow same-named constants and variants in pattern position. Because we
        // are still in pending-id space, only earlier active bindings can participate.
        if self.previous_active_binding_visible(binding_data.scope, binding, name, active) {
            return Ok(false);
        }

        let path = Path::unqualified_name(name);
        if self.pattern_path_resolves_to_const_like(binding_data.scope, &path)? {
            return Ok(true);
        }

        let (resolution, _) =
            BodyValuePathResolver::new(self.query_source(), Some(self.semantic_index))
                .resolve_nonlocal_path_expr(binding_data.scope, &path)?;

        Ok(matches!(
            resolution,
            BodyResolution::Declarations(declarations)
                if declarations.iter().any(Self::declaration_is_const_like_pattern)
        ))
    }

    fn pattern_path_resolves_to_const_like(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<bool, PackageStoreError> {
        // Check the body-local lexical module first, then the owner/fallback modules used by
        // ordinary body lookup. This keeps body-local consts visible without losing target items.
        let from = ModuleRef {
            origin: DefMapRef::Body(self.body_ref),
            module: ModuleId(scope.0),
        };
        let def_maps = DefMapQuery::new(self.query_source());
        let body_defs = def_maps
            .resolve_lexical_path(from, path, NameResolutionFilter::ValuesOnly)?
            .resolved;
        if self.semantic_items_include_const_like(body_defs)? {
            return Ok(true);
        }

        let owner_module = self.body.owner_module();
        let owner_defs = def_maps.resolve_path(owner_module, path)?.resolved;
        if self.semantic_items_include_const_like(owner_defs)? {
            return Ok(true);
        }

        let fallback_module = self.body.fallback_module();
        if fallback_module == owner_module {
            return Ok(false);
        }

        let fallback_defs = def_maps.resolve_path(fallback_module, path)?.resolved;
        self.semantic_items_include_const_like(fallback_defs)
    }

    fn semantic_items_include_const_like(
        &self,
        defs: Vec<DefId>,
    ) -> Result<bool, PackageStoreError> {
        let item_query = self.item_query();
        for def in defs {
            let DefId::Local(local_def) = def else {
                continue;
            };
            let Some(item) = item_query.semantic_item_for_local_def(local_def)? else {
                continue;
            };
            if Self::semantic_item_is_const_like_pattern(item) {
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn semantic_item_is_const_like_pattern(item: SemanticItemRef) -> bool {
        matches!(item, SemanticItemRef::Const(_) | SemanticItemRef::Static(_))
    }

    fn previous_active_binding_visible(
        &self,
        start_scope: ScopeId,
        binding: BindingId,
        name: &str,
        active: &[bool],
    ) -> bool {
        let mut scope = Some(start_scope);
        while let Some(scope_id) = scope {
            let Some(scope_data) = self.body.scope(scope_id) else {
                return false;
            };

            for candidate in scope_data.bindings.iter().rev() {
                if candidate.0 >= binding.0 {
                    continue;
                }
                if !active.get(candidate.0).copied().unwrap_or(false) {
                    continue;
                }

                let Some(candidate_data) = self.body.binding(*candidate) else {
                    continue;
                };
                if candidate_data.name.as_deref() == Some(name) {
                    return true;
                }
            }

            scope = scope_data.parent;
        }

        false
    }

    fn declaration_is_const_like_pattern(declaration: &DeclarationRef) -> bool {
        matches!(
            declaration,
            DeclarationRef::Item(SemanticItemRef::Const(_) | SemanticItemRef::Static(_))
                | DeclarationRef::EnumVariant(_)
        )
    }

    fn binding_ty(&self, binding: BindingId) -> Result<Ty, PackageStoreError> {
        let binding_data = self.body.binding_unchecked(binding);
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

    fn semantic_type_subst(&self, ty: &NominalTy) -> Result<TypeSubst, PackageStoreError> {
        Ok(self
            .item_query()
            .generic_params_for_type_def(ty.def)?
            .map(|generics| TypeSubst::from_generics(generics, &ty.args))
            .unwrap_or_else(TypeSubst::new))
    }

    fn field_item<'a>(fields: &'a FieldList, key: &FieldKey) -> Option<&'a FieldItem> {
        match key {
            FieldKey::Named(_) => fields
                .fields()
                .iter()
                .find(|field| field.key.as_ref() == Some(key)),
            FieldKey::Tuple(index) => fields
                .fields()
                .get(*index)
                .filter(|field| field.key.as_ref() == Some(key)),
        }
    }

    fn pattern_path_last_name(path: Option<&Path>) -> Option<&str> {
        match path?.segments.last()? {
            PathSegment::Name(name) => Some(name),
            PathSegment::SelfKw
            | PathSegment::SuperKw
            | PathSegment::CrateKw
            | PathSegment::DollarCrate(_) => None,
        }
    }
}
