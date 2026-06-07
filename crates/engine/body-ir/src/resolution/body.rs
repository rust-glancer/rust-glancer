//! Main body-resolution pass.
//!
//! This module walks lowered bodies and fills resolution/type slots on bindings and expressions.
//! Specialized helpers live in sibling modules so this file can read like the pass itself.

use rg_ir_model::{
    AssocItemId, BindingId, BodyRef, ConstRef, DefId, DefMapRef, ExprId, ImplRef, ItemOwner,
    ModuleId, ModuleRef, ScopeId, SemanticItemRef, StaticRef, TraitImplRef, TypeDefId,
    TypePathResolution, identity::DeclarationRef,
};
use rg_ir_storage::{
    DefMapQuery, DefMapSource, ItemLookupIndex, ItemStoreQuery, ItemStoreSource,
    NameResolutionFilter, Path, PathSegment, ResolvePathResult, TargetItemQuery, TypePathContext,
};
use rg_package_store::PackageStoreError;
use rg_ty::{Autoderef, ImplMatcher, ItemPathQuery, NominalTy, Ty, TypeSubst};

use crate::{
    ir::body::BodyData,
    ir::resolved::BodyResolution,
    ir::stmt::{BindingKind, BodySelfParamKind},
};

use super::{
    BodyLocalItemQuery, BodyQuerySource, BodyReceiverFunctionQuery, TypeRefUseSite,
    expr::ExprResolver, pat::PatternTypePropagator, pat_binding::PatternBindingMaterializer,
    push_unique, type_path::BodyTypePathResolver,
};

/// Shared state for the body-resolution fixed-point pass.
///
/// Sibling resolver modules keep their logic in separate files while operating on the same body
/// facts, so the fields are scoped to `resolution` rather than hidden inside this file.
pub(crate) struct BodyResolver<'query, 'body, D, I> {
    pub(super) def_maps: &'query D,
    pub(super) item_stores: &'query I,
    pub(super) semantic_index: &'query ItemLookupIndex,
    pub(super) body_ref: BodyRef,
    pub(super) body: &'body mut BodyData,
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

    pub(super) fn type_path_resolver<'source>(
        &'source self,
    ) -> BodyTypePathResolver<'source, &'source D, &'source I> {
        BodyTypePathResolver::new(self.query_source())
    }

    pub(super) fn query_source<'source>(
        &'source self,
    ) -> BodyQuerySource<'source, &'source D, &'source I> {
        BodyQuerySource::new(self.def_maps, self.item_stores, self.body_ref, self.body)
    }

    pub(super) fn autoderef(
        &self,
    ) -> Autoderef<'_, BodyQuerySource<'_, &D, &I>, BodyQuerySource<'_, &D, &I>> {
        let source = self.query_source();
        let item_paths = ItemPathQuery::new(source, source);
        let target_items = TargetItemQuery::new(source, source, self.body_ref.target);
        Autoderef::with_index(item_paths, target_items, self.semantic_index)
    }

    pub(super) fn impl_matcher(
        &self,
    ) -> ImplMatcher<'_, BodyQuerySource<'_, &D, &I>, BodyQuerySource<'_, &D, &I>> {
        let source = self.query_source();
        let item_paths = ItemPathQuery::new(source, source);
        let target_items = TargetItemQuery::new(source, source, self.body_ref.target);
        ImplMatcher::new(item_paths, target_items)
    }

    pub(super) fn item_query(&self) -> ItemStoreQuery<'_, BodyQuerySource<'_, &D, &I>> {
        ItemStoreQuery::new(self.query_source())
    }

    pub(super) fn receiver_functions<'source>(
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
            let expr_count = self.body.exprs.len();
            {
                let mut expr_resolver = ExprResolver::new(self);
                for expr_idx in 0..expr_count {
                    changed |= expr_resolver.resolve_expr(ExprId(expr_idx))?;
                }
            }
            let binding_updates =
                PatternTypePropagator::new(self.query_source(), self.semantic_index).propagate()?;
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
            self.body.set_binding_ty(binding, ty);
        }
        Ok(())
    }

    fn apply_binding_type_updates(&mut self, updates: Vec<(BindingId, Ty)>) -> bool {
        let mut changed = false;
        for (binding, ty) in updates {
            if matches!(ty, Ty::Unknown) {
                continue;
            }

            if self.body.bindings.get(binding).is_none() {
                continue;
            };
            if !matches!(self.body.binding_ty_unchecked(binding), Ty::Unknown) {
                continue;
            }

            self.body.set_binding_ty(binding, ty);
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
                let ty = self.source.body().binding_ty_unchecked(binding).clone();
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

        // Trait associated const lookup is still receiver-driven: `Type::CONST` first proves that
        // `Type` has a visible trait impl, then reads the matching const item from that impl or
        // falls back to the trait declaration. This mirrors trait method lookup without pretending
        // to be a full trait solver.
        let mut trait_consts = Vec::new();
        let mut trait_const_tys = Vec::new();
        for nominal_ty in prefix_ty.as_nominals() {
            for (const_ref, ty) in
                self.semantic_associated_trait_value_items_for_type(nominal_ty, last_segment)?
            {
                push_unique(&mut trait_consts, const_ref);
                push_unique(&mut trait_const_tys, ty);
            }
        }

        if !trait_consts.is_empty() {
            return Ok(Some((
                BodyResolution::Declarations(
                    trait_consts.into_iter().map(DeclarationRef::from).collect(),
                ),
                unique_ty_or_unknown(trait_const_tys),
            )));
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

    fn semantic_associated_trait_value_items_for_type(
        &self,
        ty: &NominalTy,
        name: &str,
    ) -> Result<Vec<(ConstRef, Ty)>, PackageStoreError> {
        let mut items = Vec::new();

        self.push_associated_trait_value_items_for_impls(
            &mut items,
            self.body_local_items().trait_impls_for_type(ty.def)?,
            ty,
            name,
        )?;

        if ty.def.origin == DefMapRef::Body(self.source.body_ref()) {
            return Ok(items);
        }

        let source = self.source;
        let target_items = TargetItemQuery::new(source, source, self.source.body_ref().target);
        let semantic_trait_impls = match self.semantic_index {
            Some(index) => index.trait_impls_for_type(ty.def).to_vec(),
            None => target_items.trait_impls_for_type(ty.def)?,
        };
        self.push_associated_trait_value_items_for_impls(
            &mut items,
            semantic_trait_impls,
            ty,
            name,
        )?;

        Ok(items)
    }

    fn push_associated_trait_value_items_for_impls(
        &self,
        items: &mut Vec<(ConstRef, Ty)>,
        trait_impls: Vec<TraitImplRef>,
        ty: &NominalTy,
        name: &str,
    ) -> Result<(), PackageStoreError> {
        let item_query = self.item_query();
        let matcher = self.impl_matcher();
        for trait_impl in trait_impls {
            if !matcher
                .trait_impl_applicability(trait_impl, ty)?
                .is_applicable()
            {
                continue;
            }

            let Some(impl_data) = item_query.impl_data(trait_impl.impl_ref)? else {
                continue;
            };

            // Impl consts are the concrete declaration for `Type::CONST`. When the impl omits the
            // item, use the trait declaration as a best-effort source for defaulted or incomplete
            // code; const signatures do not preserve whether a default body was written.
            let mut candidate = self.associated_const_from_items(
                trait_impl.impl_ref.origin,
                &impl_data.items,
                ty,
                name,
            )?;
            if candidate.is_none()
                && let Some(trait_data) = item_query.trait_data(trait_impl.trait_ref)?
            {
                candidate = self.associated_const_from_items(
                    trait_impl.trait_ref.origin,
                    &trait_data.items,
                    ty,
                    name,
                )?;
            }

            let Some(candidate) = candidate else {
                continue;
            };
            if !items.iter().any(|(existing, _)| *existing == candidate.0) {
                items.push(candidate);
            }
        }

        Ok(())
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

            if let Some(item) =
                self.associated_const_from_items(impl_ref.origin, &impl_data.items, ty, name)?
            {
                return Ok(Some(item));
            }
        }

        Ok(None)
    }

    fn associated_const_from_items(
        &self,
        origin: DefMapRef,
        assoc_items: &[AssocItemId],
        receiver_ty: &NominalTy,
        name: &str,
    ) -> Result<Option<(ConstRef, Ty)>, PackageStoreError> {
        let item_query = self.item_query();
        for item in assoc_items {
            let AssocItemId::Const(id) = item else {
                continue;
            };
            let const_ref = ConstRef { origin, id: *id };
            let Some(const_data) = item_query.const_data(const_ref)? else {
                continue;
            };
            if const_data.name == name {
                return Ok(Some((
                    const_ref,
                    self.semantic_const_ty_for_receiver(const_ref, const_data.owner, receiver_ty)?,
                )));
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
