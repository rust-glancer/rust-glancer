//! Type-path resolution with body-local scope awareness.
//!
//! Semantic IR can resolve module items, but body-local structs live in lexical scopes. This
//! resolver checks those scopes first and then falls back to the semantic/def-map context.

use rg_ir_model::{
    AssocItemId, DefId, DefMapRef, FunctionRef, ImplRef, ItemOwner, ModuleId, ModuleRef, ScopeId,
    SemanticItemRef, TypeAliasRef, TypeDefRef, TypePathResolution,
};
use rg_ir_storage::{
    DefMapQuery, DefMapSource, ItemStoreQuery, ItemStoreSource, NameResolutionFilter, Path,
    PathSegment, TargetItemQuery, TypePathContext,
};
use rg_item_tree::{GenericArg as ItemGenericArg, TypePath, TypeRef};
use rg_package_store::PackageStoreError;
use rg_ty::{GenericArg, ImplMatcher, ItemPathQuery, NominalTy, Ty, TypeSubst};

use super::{BodyQuerySource, push_unique};

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
            let mut type_defs = Vec::new();
            let mut type_aliases = Vec::new();
            let mut traits = Vec::new();
            for item in body_items {
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
                return Ok(TypePathResolution::TypeDefs(type_defs));
            }
            if !type_aliases.is_empty() {
                return Ok(TypePathResolution::TypeAliases(type_aliases));
            }
            if !traits.is_empty() {
                return Ok(TypePathResolution::Traits(traits));
            }
        }

        let context =
            self.context_for_function(self.source.body().owner, self.source.body().owner_module)?;
        let source = self.source;
        ItemPathQuery::new(source, source).resolve_type_path(context, path)
    }

    pub(super) fn resolve_type_ref_in_scope(
        &self,
        ty: &TypeRef,
        scope: ScopeId,
    ) -> Result<Ty, PackageStoreError> {
        self.resolve_type_ref_in_scope_with_subst(ty, scope, &TypeSubst::new())
    }

    pub(super) fn resolve_type_ref_in_scope_with_subst(
        &self,
        ty: &TypeRef,
        scope: ScopeId,
        subst: &TypeSubst,
    ) -> Result<Ty, PackageStoreError> {
        // Path types are the only type syntax we resolve structurally today. Other forms stay as
        // syntax unless they have a cheap built-in representation such as `()` or `!`.
        match ty {
            TypeRef::Path(type_path) => {
                let path = Path::from_type_path(type_path);
                if let Some(name) = path.single_name()
                    && let Some(ty) = subst.type_param(name)
                {
                    return Ok(ty);
                }

                let args = self.generic_args_from_type_path_in_scope(type_path, scope, subst)?;
                if let Some(ty) =
                    self.ty_from_local_associated_type_path(type_path, &path, scope, subst, &args)?
                {
                    return Ok(ty);
                }

                let resolution = self.resolve_in_scope(scope, &path)?;
                if let TypePathResolution::TypeAliases(aliases) = &resolution {
                    return self.ty_from_type_aliases(aliases, &args, subst);
                }
                let is_unknown = matches!(resolution, TypePathResolution::Unknown);
                Ok(
                    Ty::from_type_path_resolution(resolution, args).unwrap_or_else(|| {
                        if is_unknown {
                            path.single_name()
                                .and_then(rg_ty::PrimitiveTy::from_name)
                                .map(Ty::Primitive)
                                .unwrap_or_else(|| Ty::syntax(ty.clone()))
                        } else {
                            Ty::syntax(ty.clone())
                        }
                    }),
                )
            }
            _ => self.resolve_type_ref_in_context(
                ty,
                self.context_for_function(
                    self.source.body().owner,
                    self.source.body().owner_module,
                )?,
                subst,
            ),
        }
    }

    pub(super) fn resolve_type_ref_for_function_with_subst(
        &self,
        ty: &TypeRef,
        function: FunctionRef,
        subst: &TypeSubst,
    ) -> Result<Ty, PackageStoreError> {
        let context = self.context_for_function(function, self.source.body().owner_module)?;
        if context.module.origin == DefMapRef::Body(self.source.body_ref()) {
            return self.resolve_type_ref_in_module_with_subst(ty, context.module, subst);
        }

        self.resolve_type_ref_in_context_with_subst(ty, context, subst)
    }

    fn resolve_type_ref_in_context(
        &self,
        ty: &TypeRef,
        context: TypePathContext,
        subst: &TypeSubst,
    ) -> Result<Ty, PackageStoreError> {
        self.resolve_type_ref_in_context_with_subst(ty, context, subst)
    }

    pub(super) fn resolve_type_ref_in_context_with_subst(
        &self,
        ty: &TypeRef,
        context: TypePathContext,
        subst: &TypeSubst,
    ) -> Result<Ty, PackageStoreError> {
        let source = self.source;
        let item_paths = ItemPathQuery::new(source, source);
        item_paths.resolve_type_ref(ty, context, Ty::syntax(ty.clone()), subst)
    }

    pub(super) fn resolve_type_ref_in_module_with_subst(
        &self,
        ty: &TypeRef,
        module: ModuleRef,
        subst: &TypeSubst,
    ) -> Result<Ty, PackageStoreError> {
        // Body DefMaps allocate synthetic scope modules first, in ScopeId order. Named inline
        // modules may have ids outside that range, and the legacy body resolver did not model
        // their expression scopes either.
        if module.origin == DefMapRef::Body(self.source.body_ref()) {
            let scope = ScopeId(module.module.0);
            if self.source.body().scope(scope).is_some() {
                return self.resolve_type_ref_in_scope_with_subst(ty, scope, subst);
            }
        }

        self.resolve_type_ref_in_context_with_subst(ty, TypePathContext::module(module), subst)
    }

    pub(super) fn self_tys_for_function(
        &self,
        function: FunctionRef,
    ) -> Result<Vec<TypeDefRef>, PackageStoreError> {
        // `self` parameters and explicit `Self` annotations need the enclosing impl owner, not
        // just the owner module. Semantic IR owns that function-to-owner mapping.
        let Some(impl_ref) = self
            .context_for_function(function, self.source.body().owner_module)?
            .impl_ref
        else {
            return Ok(Vec::new());
        };

        Ok(self
            .item_query()
            .impl_data(impl_ref)?
            .map(|impl_data| impl_data.resolved_self_tys.clone())
            .unwrap_or_default())
    }

    fn context_for_function(
        &self,
        function: FunctionRef,
        fallback_module: ModuleRef,
    ) -> Result<TypePathContext, PackageStoreError> {
        Ok(self
            .item_query()
            .type_path_context_for_function(function)?
            .unwrap_or_else(|| TypePathContext::module(fallback_module)))
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

        let mut items = Vec::new();
        for def in result.resolved {
            let DefId::Local(local_def) = def else {
                continue;
            };
            if let Some(item) = self.item_query().semantic_item_for_local_def(local_def)? {
                push_unique(&mut items, item);
            }
        }

        Ok(items)
    }

    fn ty_from_local_associated_type_path(
        &self,
        type_path: &TypePath,
        path: &Path,
        scope: ScopeId,
        subst: &TypeSubst,
        args: &[GenericArg],
    ) -> Result<Option<Ty>, PackageStoreError> {
        let Some((_, name)) = split_associated_path(path) else {
            return Ok(None);
        };
        let Some(prefix_ty_ref) = prefix_type_ref(type_path) else {
            return Ok(None);
        };
        let prefix_ty = self.resolve_type_ref_in_scope_with_subst(&prefix_ty_ref, scope, subst)?;

        for ty in prefix_ty.as_nominals() {
            let Some(alias_ref) = self.associated_type_alias_for_type(ty, name)? else {
                continue;
            };
            return self
                .ty_from_associated_type_alias(alias_ref, ty, args)
                .map(Some);
        }

        Ok(None)
    }

    fn ty_from_type_aliases(
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
            .unwrap_or_else(|| TypePathContext::module(self.source.body().owner_module));
        if context.module.origin == DefMapRef::Body(self.source.body_ref()) {
            self.resolve_type_ref_in_module_with_subst(aliased_ty, context.module, &alias_subst)
        } else {
            self.resolve_type_ref_in_context_with_subst(aliased_ty, context, &alias_subst)
        }
    }

    fn associated_type_alias_for_type(
        &self,
        ty: &NominalTy,
        name: &str,
    ) -> Result<Option<TypeAliasRef>, PackageStoreError> {
        if ty.def.origin != DefMapRef::Body(self.source.body_ref()) {
            return Ok(None);
        }

        let item_query = self.item_query();
        let source = self.source;
        let target_items = TargetItemQuery::new(source, source, self.source.body_ref().target);
        for impl_ref in target_items.inherent_impls_for_type(ty.def)? {
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

    fn ty_from_associated_type_alias(
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
            .unwrap_or_else(|| TypePathContext::module(self.source.body().owner_module));
        if context.module.origin == DefMapRef::Body(self.source.body_ref()) {
            self.resolve_type_ref_in_module_with_subst(aliased_ty, context.module, &alias_subst)
        } else {
            self.resolve_type_ref_in_context_with_subst(aliased_ty, context, &alias_subst)
        }
    }

    fn semantic_type_subst(&self, ty: &NominalTy) -> Result<TypeSubst, PackageStoreError> {
        Ok(self
            .item_query()
            .generic_params_for_type_def(ty.def)?
            .map(|generics| TypeSubst::from_generics(generics, &ty.args))
            .unwrap_or_else(TypeSubst::new))
    }

    fn generic_args_from_type_path_in_scope(
        &self,
        type_path: &rg_item_tree::TypePath,
        scope: ScopeId,
        subst: &TypeSubst,
    ) -> Result<Vec<GenericArg>, PackageStoreError> {
        let Some(segment) = type_path.segments.last() else {
            return Ok(Vec::new());
        };
        self.generic_args_from_item_tree_args_in_scope(&segment.args, scope, subst)
    }

    fn generic_args_from_item_tree_args_in_scope(
        &self,
        args: &[ItemGenericArg],
        scope: ScopeId,
        subst: &TypeSubst,
    ) -> Result<Vec<GenericArg>, PackageStoreError> {
        let mut generic_args = Vec::new();
        for arg in args {
            generic_args.push(self.generic_arg_from_item_tree_arg_in_scope(arg, scope, subst)?);
        }
        Ok(generic_args)
    }

    fn generic_arg_from_item_tree_arg_in_scope(
        &self,
        arg: &ItemGenericArg,
        scope: ScopeId,
        subst: &TypeSubst,
    ) -> Result<GenericArg, PackageStoreError> {
        match arg {
            ItemGenericArg::Type(ty) => Ok(GenericArg::Type(Box::new(
                self.resolve_type_ref_in_scope_with_subst(ty, scope, subst)?,
            ))),
            ItemGenericArg::Lifetime(lifetime) => Ok(GenericArg::Lifetime(lifetime.clone())),
            ItemGenericArg::Const(value) => Ok(GenericArg::Const(value.clone())),
            ItemGenericArg::AssocType { name, ty } => Ok(GenericArg::AssocType {
                name: name.clone(),
                ty: match ty {
                    Some(ty) => Some(Box::new(
                        self.resolve_type_ref_in_scope_with_subst(ty, scope, subst)?,
                    )),
                    None => None,
                },
            }),
            ItemGenericArg::Unsupported(text) => Ok(GenericArg::Unsupported(text.clone())),
        }
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

fn prefix_type_ref(path: &TypePath) -> Option<TypeRef> {
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
