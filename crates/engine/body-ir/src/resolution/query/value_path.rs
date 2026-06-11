//! Value-path lookup.

use rg_ir_model::{
    BindingId, ConstRef, DefId, DefMapRef, ModuleId, ModuleRef, Path, ScopeId, SemanticItemRef,
    StaticRef, TypePathResolution, identity::DeclarationRef,
};
use rg_ir_storage::{
    DefMapSource, ItemStoreSource, NameResolutionFilter, ResolvePathResult, TypePathContext,
};
use rg_package_store::PackageStoreError;
use rg_std::UniqueVec;
use rg_ty::{NominalTy, Ty};

use crate::ir::resolved::BodyResolution;
use crate::resolution::{BodyResolutionContext, TypeRefUseSite, support::unique_ty_or_unknown};

/// Resolves paths in the value namespace without mutating the body.
pub struct BodyValuePathQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

/// One declaration that can satisfy a value name inside a body scope.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BodyValueName {
    Binding(BindingId),
    SemanticItems(UniqueVec<SemanticItemRef>),
}

impl<'query, D, I> BodyValuePathQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Find declarations for a path without considering ordinary local bindings.
    pub fn resolve_nonlocal_path_declarations(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<Vec<DeclarationRef>, PackageStoreError> {
        let (resolution, _) = self.resolve_nonlocal_path_expr(scope, path)?;
        Ok(resolution.declarations(self.context.body_ref()))
    }

    /// Find the type of a path without considering ordinary local bindings.
    pub fn resolve_nonlocal_path_ty(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<Ty, PackageStoreError> {
        let (_, ty) = self.resolve_nonlocal_path_expr(scope, path)?;
        Ok(ty)
    }

    /// Resolve a path without considering ordinary local bindings.
    pub(crate) fn resolve_nonlocal_path_expr(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<(BodyResolution, Ty), PackageStoreError> {
        self.resolve_path_expr(scope, path, None)
    }

    /// Resolve a value path from a body scope.
    ///
    /// `visible_bindings` caps which local bindings are visible for local queries.
    pub(crate) fn resolve_path_expr(
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
        match self
            .context
            .type_path_query()
            .resolve_in_scope(scope, path)?
        {
            TypePathResolution::SelfType(types) => {
                return Ok((
                    BodyResolution::Unknown,
                    Ty::self_ty(types.into_iter().map(NominalTy::bare).collect()),
                ));
            }
            TypePathResolution::TypeDefs(types) => {
                let mut constructors = UniqueVec::new();
                for type_def in types
                    .into_iter()
                    .filter(|ty| ty.origin == DefMapRef::Body(self.context.body_ref()))
                {
                    if self
                        .context
                        .item_query()
                        .type_def_has_value_constructor(type_def)?
                    {
                        constructors.push(type_def);
                    }
                }

                if !constructors.is_empty() {
                    let declarations = constructors
                        .iter()
                        .copied()
                        .map(DeclarationRef::from)
                        .collect();
                    return Ok((
                        BodyResolution::Declarations(declarations),
                        Ty::nominal(constructors.into_iter().map(NominalTy::bare).collect()),
                    ));
                }
            }
            TypePathResolution::TypeAliases(_)
            | TypePathResolution::Traits(_)
            | TypePathResolution::Unknown => {}
        }

        if let Some((prefix, last_segment)) = path.split_prefix_name() {
            if let Some((resolution, ty)) =
                self.context
                    .associated_items()
                    .resolve_path(scope, &prefix, last_segment)?
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

    /// Search one value name through parent scopes, with an optional local binding cutoff.
    fn resolve_single_segment_value_name(
        &self,
        start_scope: ScopeId,
        name: &str,
        visible_bindings: Option<usize>,
    ) -> Result<Option<(BodyResolution, Ty)>, PackageStoreError> {
        // Value lookup is scope-ordered: an inner const/function shadows an outer binding just as
        // surely as an inner binding shadows an outer item.
        let from = ModuleRef {
            origin: DefMapRef::Body(self.context.body_ref()),
            module: ModuleId(start_scope.0),
        };
        let mut scope = Some(start_scope);
        while let Some(scope_id) = scope {
            let Some(scope_data) = self.context.body().scope(scope_id) else {
                return Ok(None);
            };

            if let Some(visible_bindings) = visible_bindings {
                for binding in scope_data.bindings.iter().rev() {
                    if binding.0 >= visible_bindings {
                        continue;
                    }

                    let Some(binding_data) = self.context.body().binding(*binding) else {
                        continue;
                    };
                    if binding_data.name.as_deref() == Some(name) {
                        return self.value_name_resolution(BodyValueName::Binding(*binding));
                    }
                }
            }

            let module = ModuleRef {
                origin: DefMapRef::Body(self.context.body_ref()),
                module: ModuleId(scope_id.0),
            };
            let defs = self
                .context
                .def_map_query()
                .resolve_lexical_name_in_module(
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

    /// Look up a path from the body owner module, then the fallback module.
    fn resolve_path_from_owner_modules(
        &self,
        path: &Path,
    ) -> Result<ResolvePathResult, PackageStoreError> {
        let owner_module = self.context.body().owner_module();
        let result = self
            .context
            .def_map_query()
            .resolve_path(owner_module, path)?;
        if !result.resolved.is_empty() {
            return Ok(result);
        }

        let fallback_module = self.context.body().fallback_module();
        if fallback_module == owner_module {
            return Ok(result);
        }

        self.context
            .def_map_query()
            .resolve_path(fallback_module, path)
    }

    /// Resolve a multi-segment value path through the body def map.
    fn resolve_body_value_path_from_def_map(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<Option<(BodyResolution, Ty)>, PackageStoreError> {
        let from = ModuleRef {
            origin: DefMapRef::Body(self.context.body_ref()),
            module: ModuleId(scope.0),
        };
        let defs = self
            .context
            .def_map_query()
            .resolve_lexical_path(from, path, NameResolutionFilter::ValuesOnly)?
            .resolved;
        self.value_name_resolution(BodyValueName::SemanticItems(
            self.semantic_items_for_defs(defs)?,
        ))
    }

    /// Keep only semantic items that belong to the value namespace.
    fn semantic_items_for_defs(
        &self,
        defs: Vec<DefId>,
    ) -> Result<UniqueVec<SemanticItemRef>, PackageStoreError> {
        let mut items = UniqueVec::new();
        for def in defs {
            let DefId::Local(local_def) = def else {
                continue;
            };
            let Some(item) = self
                .context
                .item_query()
                .semantic_item_for_local_def(local_def)?
            else {
                continue;
            };
            if matches!(
                item,
                SemanticItemRef::Function(_)
                    | SemanticItemRef::Const(_)
                    | SemanticItemRef::Static(_)
            ) {
                items.push(item);
            }
        }

        Ok(items)
    }

    /// Convert one value-namespace match into body resolution and type.
    fn value_name_resolution(
        &self,
        value_name: BodyValueName,
    ) -> Result<Option<(BodyResolution, Ty)>, PackageStoreError> {
        match value_name {
            BodyValueName::Binding(binding) => {
                let ty = self.context.body().binding_ty_unchecked(binding).clone();
                Ok(Some((BodyResolution::Binding(binding), ty)))
            }
            BodyValueName::SemanticItems(items) => {
                let mut functions = UniqueVec::new();
                let mut declarations = UniqueVec::new();
                let mut tys = UniqueVec::new();

                for item in items {
                    match item {
                        SemanticItemRef::Function(function) => {
                            functions.push(DeclarationRef::from(function));
                        }
                        SemanticItemRef::Const(const_ref) => {
                            declarations.push(DeclarationRef::from(const_ref));
                            tys.push(self.semantic_const_ty(const_ref)?);
                        }
                        SemanticItemRef::Static(static_ref) => {
                            declarations.push(DeclarationRef::from(static_ref));
                            tys.push(self.semantic_static_ty(static_ref)?);
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

    /// Resolve the declared type of a const item.
    fn semantic_const_ty(&self, const_ref: ConstRef) -> Result<Ty, PackageStoreError> {
        let item_query = self.context.item_query();
        let Some(const_data) = item_query.const_data(const_ref)? else {
            return Ok(Ty::Unknown);
        };
        let Some(ty) = const_data.signature.ty() else {
            return Ok(Ty::Unknown);
        };

        let context = item_query
            .type_path_context_for_owner(const_ref.origin, const_data.owner)?
            .unwrap_or_else(|| TypePathContext::module(self.context.body().owner_module()));
        self.context
            .type_refs(TypeRefUseSite::OwnerContext(context))
            .resolve(ty)
    }

    /// Resolve the declared type of a static item.
    fn semantic_static_ty(&self, static_ref: StaticRef) -> Result<Ty, PackageStoreError> {
        let item_query = self.context.item_query();
        let Some(static_data) = item_query.static_data(static_ref)? else {
            return Ok(Ty::Unknown);
        };
        let Some(ty) = &static_data.ty else {
            return Ok(Ty::Unknown);
        };

        self.context
            .type_refs(TypeRefUseSite::Module(static_data.owner))
            .resolve(ty)
    }

    /// Turn type-def declarations into a nominal type.
    fn nominal_ty_from_defs(&self, defs: &[DefId]) -> Result<Ty, PackageStoreError> {
        let mut type_defs = UniqueVec::new();
        for def in defs {
            let DefId::Local(local_def) = def else {
                continue;
            };
            let Some(SemanticItemRef::TypeDef(type_def)) = self
                .context
                .item_query()
                .semantic_item_for_local_def(*local_def)?
            else {
                continue;
            };
            type_defs.push(type_def);
        }

        Ok(if type_defs.is_empty() {
            Ty::Unknown
        } else {
            Ty::nominal(type_defs.into_iter().map(NominalTy::bare).collect())
        })
    }
}
