//! Type-path resolution with body-local scope awareness.
//!
//! Semantic IR can resolve module items, but body-local structs live in lexical scopes. This
//! resolver checks those scopes first and then falls back to the semantic/def-map context.

use rg_ir_model::{
    AssocItemId, DefId, DefMapRef, FunctionRef, ImplRef, ItemOwner, ModuleId, ModuleRef, Path,
    PathSegment, ScopeId, SemanticItemRef, TypeAliasRef, TypePathResolution,
};
use rg_ir_storage::{
    DefMapQuery, DefMapSource, ItemStoreQuery, ItemStoreSource, NameResolutionFilter,
    TargetItemQuery, TypePathContext,
};
use rg_item_tree::{TypePath, TypeRef};
use rg_package_store::PackageStoreError;
use rg_ty::{GenericArg, ImplMatcher, ItemPathQuery, NominalTy, Ty, TypeSubst};

use crate::ir::BodyOwner;

use super::{
    BodyLocalItemQuery, BodyQuerySource, push_unique,
    type_ref::{TypeRefResolutionQuery, TypeRefUseSite},
};

pub(crate) struct BodyTypePathResolver<'query, D, I> {
    source: BodyQuerySource<'query, D, I>,
}

impl<'query, D, I> BodyTypePathResolver<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(source: BodyQuerySource<'query, D, I>) -> Self {
        Self { source }
    }

    pub(crate) fn type_ref(
        &self,
        use_site: TypeRefUseSite,
    ) -> TypeRefResolutionQuery<'_, 'query, D, I> {
        TypeRefResolutionQuery::new(self, use_site)
    }

    pub(super) fn source(&self) -> BodyQuerySource<'query, D, I> {
        self.source
    }

    fn impl_matcher(
        &self,
    ) -> ImplMatcher<'query, BodyQuerySource<'query, D, I>, BodyQuerySource<'query, D, I>> {
        let source = self.source;
        let item_paths = ItemPathQuery::new(source, source);
        let target_items = TargetItemQuery::new(source, source, self.source.body_ref().target);
        ImplMatcher::new(item_paths, target_items)
    }

    pub(super) fn item_query(&self) -> ItemStoreQuery<'query, BodyQuerySource<'query, D, I>> {
        ItemStoreQuery::new(self.source)
    }

    fn body_local_items(&self) -> BodyLocalItemQuery<'query, D, I> {
        BodyLocalItemQuery::new(self.source)
    }

    pub(crate) fn resolve_in_scope(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<TypePathResolution, PackageStoreError> {
        if let Some((prefix, name)) = split_associated_path(path) {
            let prefix_resolution = self.resolve_in_scope(scope, &prefix)?;
            let prefix_ty =
                Ty::from_type_path_resolution(prefix_resolution, Vec::new()).unwrap_or(Ty::Unknown);
            let mut aliases = Vec::new();
            for ty in prefix_ty.as_nominals() {
                if let Some(alias) = self.associated_type_alias_for_type(ty, name)? {
                    push_unique(&mut aliases, alias);
                }
            }
            if !aliases.is_empty() {
                return Ok(TypePathResolution::TypeAliases(aliases));
            }
        }

        let body_items = self.resolve_body_type_items_from_def_map(scope, path)?;
        if !body_items.is_empty() {
            return Ok(self.type_resolution_from_items(body_items));
        }

        let source = self.source;
        let item_paths = ItemPathQuery::new(source, source);
        let context = self.context_for_body_owner()?;
        let resolution = item_paths.resolve_type_path(context, path)?;
        if !matches!(resolution, TypePathResolution::Unknown) {
            return Ok(resolution);
        }

        let fallback_module = self.source.body().fallback_module();
        if fallback_module == context.module {
            return Ok(resolution);
        }

        item_paths.resolve_type_path(
            TypePathContext {
                module: fallback_module,
                impl_ref: context.impl_ref,
            },
            path,
        )
    }

    pub(super) fn self_nominal_tys_for_function(
        &self,
        function: FunctionRef,
    ) -> Result<Vec<NominalTy>, PackageStoreError> {
        let context = self.context_for_function(function, self.source.body().owner_module())?;
        self.self_nominal_tys_for_context(context)
    }

    pub(super) fn self_nominal_tys_for_context(
        &self,
        context: TypePathContext,
    ) -> Result<Vec<NominalTy>, PackageStoreError> {
        let Some(impl_ref) = context.impl_ref else {
            return Ok(Vec::new());
        };
        let item_query = self.item_query();
        let Some(impl_data) = item_query.impl_data(impl_ref)? else {
            return Ok(Vec::new());
        };

        let source = self.source;
        let item_paths = ItemPathQuery::new(source, source);
        let resolved = item_paths.resolve_type_ref(
            &impl_data.self_ty,
            context,
            Ty::Unknown,
            &TypeSubst::new(),
        )?;

        let mut self_tys = Vec::new();
        for ty in resolved.as_nominals() {
            if impl_data.resolved_self_tys.contains(&ty.def) {
                push_unique(&mut self_tys, ty.clone());
            }
        }

        if self_tys.is_empty() {
            self_tys.extend(
                impl_data
                    .resolved_self_tys
                    .iter()
                    .copied()
                    .map(NominalTy::bare),
            );
        }

        Ok(self_tys)
    }

    pub(super) fn context_for_function(
        &self,
        function: FunctionRef,
        fallback_module: ModuleRef,
    ) -> Result<TypePathContext, PackageStoreError> {
        Ok(self
            .item_query()
            .type_path_context_for_function(function)?
            .unwrap_or_else(|| TypePathContext::module(fallback_module)))
    }

    pub(super) fn context_for_body_owner(&self) -> Result<TypePathContext, PackageStoreError> {
        let fallback_module = self.source.body().owner_module();
        match self.source.body().owner() {
            BodyOwner::Function(function) => self.context_for_function(function, fallback_module),
            BodyOwner::Const(const_ref) => {
                let item_query = self.item_query();
                let Some(data) = item_query.const_data(const_ref)? else {
                    return Ok(TypePathContext::module(fallback_module));
                };
                item_query
                    .type_path_context_for_owner(const_ref.origin, data.owner)?
                    .map_or_else(|| Ok(TypePathContext::module(fallback_module)), Ok)
            }
            BodyOwner::Static(_) => Ok(TypePathContext::module(fallback_module)),
        }
    }

    fn resolve_body_type_items_from_def_map(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<Vec<SemanticItemRef>, PackageStoreError> {
        let from = ModuleRef {
            origin: DefMapRef::Body(self.source.body_ref()),
            module: ModuleId(scope.0),
        };
        let result = DefMapQuery::new(self.source).resolve_lexical_path(
            from,
            path,
            NameResolutionFilter::TypesOnly,
        )?;

        self.semantic_items_for_defs(result.resolved)
    }

    pub(super) fn resolve_body_type_items_from_module(
        &self,
        module: ModuleRef,
        path: &Path,
    ) -> Result<Vec<SemanticItemRef>, PackageStoreError> {
        let def_maps = DefMapQuery::new(self.source);
        let result = def_maps.resolve_path_in_type_namespace(module, path)?;
        let items = self.semantic_items_for_defs(result.resolved)?;
        if !items.is_empty() {
            return Ok(items);
        }

        // A body-local module only carries the lexical body facts. The inherited fallback keeps
        // signatures on parent body-local items able to name ordinary surrounding module items.
        let fallback_module = self.source.body().fallback_module();
        if fallback_module == module {
            return Ok(items);
        }

        let result = def_maps.resolve_path_in_type_namespace(fallback_module, path)?;
        self.semantic_items_for_defs(result.resolved)
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
            if let Some(item) = self.item_query().semantic_item_for_local_def(local_def)? {
                push_unique(&mut items, item);
            }
        }
        Ok(items)
    }

    pub(super) fn type_resolution_from_items(
        &self,
        items: Vec<SemanticItemRef>,
    ) -> TypePathResolution {
        let mut type_defs = Vec::new();
        let mut type_aliases = Vec::new();
        let mut traits = Vec::new();
        for item in items {
            match item {
                SemanticItemRef::TypeDef(type_def) => push_unique(&mut type_defs, type_def),
                SemanticItemRef::TypeAlias(type_alias) => {
                    push_unique(&mut type_aliases, type_alias);
                }
                SemanticItemRef::Trait(trait_ref) => push_unique(&mut traits, trait_ref),
                SemanticItemRef::Impl(_)
                | SemanticItemRef::Function(_)
                | SemanticItemRef::Const(_)
                | SemanticItemRef::Static(_) => {}
            }
        }

        if !type_defs.is_empty() {
            TypePathResolution::TypeDefs(type_defs)
        } else if !type_aliases.is_empty() {
            TypePathResolution::TypeAliases(type_aliases)
        } else if !traits.is_empty() {
            TypePathResolution::Traits(traits)
        } else {
            TypePathResolution::Unknown
        }
    }

    pub(super) fn ty_from_type_aliases(
        &self,
        aliases: &[TypeAliasRef],
        args: &[GenericArg],
        subst: &TypeSubst,
    ) -> Result<Ty, PackageStoreError> {
        if aliases.len() != 1 {
            return Ok(Ty::Unknown);
        }

        self.ty_from_type_alias(
            aliases
                .first()
                .copied()
                .expect("one alias should exist after length check"),
            args,
            subst,
        )
    }

    fn ty_from_type_alias(
        &self,
        alias_ref: TypeAliasRef,
        args: &[GenericArg],
        subst: &TypeSubst,
    ) -> Result<Ty, PackageStoreError> {
        let item_query = self.item_query();
        let Some(alias_data) = item_query.type_alias_data(alias_ref)? else {
            return Ok(Ty::Unknown);
        };
        let Some(aliased_ty) = alias_data.signature.aliased_ty() else {
            return Ok(Ty::Unknown);
        };
        if aliased_ty.is_self_type() {
            return Ok(Ty::Unknown);
        }

        let mut alias_subst = subst.clone();
        if let Some(generics) = alias_data.signature.generics() {
            alias_subst.extend(TypeSubst::from_generics(generics, args));
        }

        let context = item_query
            .type_path_context_for_owner(alias_ref.origin, alias_data.owner)?
            .unwrap_or_else(|| TypePathContext::module(self.source.body().owner_module()));
        self.type_ref(TypeRefUseSite::OwnerContext(context))
            .with_subst(&alias_subst)
            .resolve(aliased_ty)
    }

    /// Attempts to find the type alias with given name for the provided type
    /// in the body-local context.
    pub(super) fn associated_type_alias_for_type(
        &self,
        ty: &NominalTy,
        name: &str,
    ) -> Result<Option<TypeAliasRef>, PackageStoreError> {
        // In order to find the associated type, we need to iterate through its impl
        // blocks.
        let impls = self.body_local_items().inherent_impls_for_type(ty.def)?;

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
                let AssocItemId::TypeAlias(id) = item else {
                    continue;
                };
                let alias_ref = TypeAliasRef {
                    origin: impl_ref.origin,
                    id: *id,
                };
                let Some(alias_data) = item_query.type_alias_data(alias_ref)? else {
                    continue;
                };
                if alias_data.name == name {
                    return Ok(Some(alias_ref));
                }
            }
        }

        Ok(None)
    }

    pub(super) fn ty_from_associated_type_alias(
        &self,
        alias_ref: TypeAliasRef,
        receiver_ty: &NominalTy,
        args: &[GenericArg],
    ) -> Result<Ty, PackageStoreError> {
        let item_query = self.item_query();
        let Some(alias_data) = item_query.type_alias_data(alias_ref)? else {
            return Ok(Ty::Unknown);
        };
        let Some(aliased_ty) = alias_data.signature.aliased_ty() else {
            return Ok(Ty::Unknown);
        };
        if aliased_ty.is_self_type() {
            return Ok(Ty::nominal(vec![receiver_ty.clone()]));
        }

        let mut alias_subst = self.semantic_type_subst(receiver_ty)?;
        if let ItemOwner::Impl(impl_id) = alias_data.owner {
            let impl_ref = ImplRef {
                origin: alias_ref.origin,
                id: impl_id,
            };
            if let Some(impl_data) = item_query.impl_data(impl_ref)? {
                alias_subst.extend(
                    self.impl_matcher()
                        .impl_self_subst_for_impl(impl_data, receiver_ty),
                );
            }
        }
        if let Some(generics) = alias_data.signature.generics() {
            alias_subst.extend(TypeSubst::from_generics(generics, args));
        }

        let context = item_query
            .type_path_context_for_owner(alias_ref.origin, alias_data.owner)?
            .unwrap_or_else(|| TypePathContext::module(self.source.body().owner_module()));
        self.type_ref(TypeRefUseSite::OwnerContext(context))
            .with_subst(&alias_subst)
            .resolve(aliased_ty)
    }

    pub(super) fn semantic_type_subst(
        &self,
        ty: &NominalTy,
    ) -> Result<TypeSubst, PackageStoreError> {
        Ok(self
            .item_query()
            .generic_params_for_type_def(ty.def)?
            .map(|generics| TypeSubst::from_generics(generics, &ty.args))
            .unwrap_or_else(TypeSubst::new))
    }
}

pub(super) fn split_associated_path(path: &Path) -> Option<(Path, &str)> {
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

pub(super) fn prefix_type_ref(path: &TypePath) -> Option<TypeRef> {
    let prefix_len = path.segments.len().checked_sub(1)?;
    if prefix_len == 0 {
        return None;
    }

    Some(TypeRef::Path(TypePath {
        source_span: path.source_span,
        absolute: path.absolute,
        segments: path.segments[..prefix_len].to_vec(),
    }))
}
