//! Type-path resolution with body-local scope awareness.
//!
//! Semantic IR can resolve module items, but body-local structs live in lexical scopes. This
//! resolver checks those scopes first and then falls back to the semantic/def-map context.

use rg_def_map::{DefMapReadTxn, Path, PathSegment};
use rg_ir_model::{
    AssocItemId, BodyRef, DefId, DefMapRef, FunctionRef, ImplRef, ItemOwner, ModuleId, ModuleRef,
    ScopeId, SemanticItemRef, TypeAliasRef, TypeDefRef,
};
use rg_item_tree::{GenericArg, TypePath, TypeRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{SemanticIrReadTxn, TypePathContext};
use rg_ty::{
    IndexedGenericArg, IndexedNominalTy, IndexedTy, IndexedTyExt, IndexedTyRepr, IndexedTypeSubst,
};

use crate::{ir::body::BodyData, ir::resolved::BodyTypePathResolution};

use super::{
    def_map_lookup::BodyDefMapLookup,
    impl_match::BodyImplMatcher,
    item_query::BodyItemQuery,
    push_unique,
    ty::{
        subst_from_generics, substitute_type_param, ty_from_body_resolution,
        ty_from_type_ref_in_context, type_ref_is_self,
    },
};

pub(crate) struct BodyTypePathResolver<'query, 'db, 'body> {
    def_map: &'query DefMapReadTxn<'db>,
    semantic_ir: &'query SemanticIrReadTxn<'db>,
    body_ref: BodyRef,
    body: &'body BodyData,
}

impl<'query, 'db, 'body> BodyTypePathResolver<'query, 'db, 'body> {
    pub(crate) fn new(
        def_map: &'query DefMapReadTxn<'db>,
        semantic_ir: &'query SemanticIrReadTxn<'db>,
        body_ref: BodyRef,
        body: &'body BodyData,
    ) -> Self {
        Self {
            def_map,
            semantic_ir,
            body_ref,
            body,
        }
    }

    fn semantic_impl_matcher(&self) -> BodyImplMatcher<'_, 'db> {
        BodyImplMatcher::new(self.def_map, self.semantic_ir)
    }

    fn item_query(&self) -> BodyItemQuery<'_, 'db, '_> {
        BodyItemQuery::new(self.semantic_ir, self.body_ref, self.body)
    }

    pub(crate) fn resolve_in_scope(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<BodyTypePathResolution, PackageStoreError> {
        if let Some((prefix, name)) = split_associated_path(path) {
            let prefix_resolution = self.resolve_in_scope(scope, &prefix)?;
            let prefix_ty =
                ty_from_body_resolution(prefix_resolution, IndexedTy::Unknown, Vec::new());
            let mut aliases = Vec::new();
            for ty in prefix_ty.as_nominals() {
                if let Some(alias) = self.associated_type_alias_for_type(ty, name)? {
                    push_unique(&mut aliases, alias);
                }
            }
            if !aliases.is_empty() {
                return Ok(BodyTypePathResolution::TypeAliases(aliases));
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
                return Ok(BodyTypePathResolution::TypeDefs(type_defs));
            }
            if !type_aliases.is_empty() {
                return Ok(BodyTypePathResolution::TypeAliases(type_aliases));
            }
            if !traits.is_empty() {
                return Ok(BodyTypePathResolution::Traits(traits));
            }
        }

        let context = self.context_for_function(self.body.owner, self.body.owner_module)?;
        let resolution = self
            .semantic_ir
            .resolve_type_path(self.def_map, context, path)?;
        let resolution = BodyTypePathResolution::from(resolution);
        if matches!(resolution, BodyTypePathResolution::Unknown)
            && let Some(primitive) = path.single_name().and_then(rg_ty::PrimitiveTy::from_name)
        {
            return Ok(BodyTypePathResolution::Primitive(primitive));
        }
        Ok(resolution)
    }

    pub(super) fn ty_from_type_ref_in_scope(
        &self,
        ty: &TypeRef,
        scope: ScopeId,
    ) -> Result<IndexedTy, PackageStoreError> {
        self.ty_from_type_ref_in_scope_with_subst(ty, scope, &IndexedTypeSubst::new())
    }

    pub(super) fn ty_from_type_ref_in_scope_with_subst(
        &self,
        ty: &TypeRef,
        scope: ScopeId,
        subst: &IndexedTypeSubst,
    ) -> Result<IndexedTy, PackageStoreError> {
        // Path types are the only type syntax we resolve structurally today. Other forms stay as
        // syntax unless they have a cheap built-in representation such as `()` or `!`.
        match ty {
            TypeRef::Path(type_path) => {
                let path = Path::from_type_path(type_path);
                if let Some(ty) = substitute_type_param(&path, subst) {
                    return Ok(ty);
                }

                let args = self.generic_args_from_type_path_in_scope(type_path, scope, subst)?;
                if let Some(ty) =
                    self.ty_from_local_associated_type_path(type_path, &path, scope, subst, &args)?
                {
                    return Ok(ty);
                }

                let resolution = self.resolve_in_scope(scope, &path)?;
                if let BodyTypePathResolution::TypeAliases(aliases) = &resolution {
                    return self.ty_from_type_aliases(aliases, &args, subst);
                }
                Ok(ty_from_body_resolution(
                    resolution,
                    IndexedTyRepr::syntax(ty.clone()),
                    args,
                ))
            }
            _ => self.ty_from_type_ref_in_context(
                ty,
                self.context_for_function(self.body.owner, self.body.owner_module)?,
                subst,
            ),
        }
    }

    pub(super) fn ty_from_type_ref_for_function_with_subst(
        &self,
        ty: &TypeRef,
        function: FunctionRef,
        subst: &IndexedTypeSubst,
    ) -> Result<IndexedTy, PackageStoreError> {
        let context = self.context_for_function(function, self.body.owner_module)?;
        if context.module.origin == DefMapRef::Body(self.body_ref) {
            return self.ty_from_type_ref_in_module_with_subst(ty, context.module, subst);
        }

        self.ty_from_type_ref_in_context_with_subst(ty, context, subst)
    }

    fn ty_from_type_ref_in_context(
        &self,
        ty: &TypeRef,
        context: TypePathContext,
        subst: &IndexedTypeSubst,
    ) -> Result<IndexedTy, PackageStoreError> {
        self.ty_from_type_ref_in_context_with_subst(ty, context, subst)
    }

    pub(super) fn ty_from_type_ref_in_context_with_subst(
        &self,
        ty: &TypeRef,
        context: TypePathContext,
        subst: &IndexedTypeSubst,
    ) -> Result<IndexedTy, PackageStoreError> {
        ty_from_type_ref_in_context(
            self.def_map,
            self.semantic_ir,
            ty,
            context,
            IndexedTyRepr::syntax(ty.clone()),
            subst,
        )
    }

    pub(super) fn ty_from_type_ref_in_module_with_subst(
        &self,
        ty: &TypeRef,
        module: ModuleRef,
        subst: &IndexedTypeSubst,
    ) -> Result<IndexedTy, PackageStoreError> {
        // Body DefMaps allocate synthetic scope modules first, in ScopeId order. Named inline
        // modules may have ids outside that range, and the legacy body resolver did not model
        // their expression scopes either.
        if module.origin == DefMapRef::Body(self.body_ref) {
            let scope = ScopeId(module.module.0);
            if self.body.scope(scope).is_some() {
                return self.ty_from_type_ref_in_scope_with_subst(ty, scope, subst);
            }
        }

        self.ty_from_type_ref_in_context_with_subst(ty, TypePathContext::module(module), subst)
    }

    pub(super) fn self_tys_for_function(
        &self,
        function: FunctionRef,
    ) -> Result<Vec<TypeDefRef>, PackageStoreError> {
        // `self` parameters and explicit `Self` annotations need the enclosing impl owner, not
        // just the owner module. Semantic IR owns that function-to-owner mapping.
        let Some(impl_ref) = self
            .context_for_function(function, self.body.owner_module)?
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
        let Some(def_map) = self.body.body_def_map() else {
            return Ok(Vec::new());
        };

        let from = ModuleRef {
            origin: DefMapRef::Body(self.body_ref),
            module: ModuleId(scope.0),
        };
        let result = BodyDefMapLookup::new(def_map).resolve_path_in_type_namespace(from, path);

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
        subst: &IndexedTypeSubst,
        args: &[IndexedGenericArg],
    ) -> Result<Option<IndexedTy>, PackageStoreError> {
        let Some((_, name)) = split_associated_path(path) else {
            return Ok(None);
        };
        let Some(prefix_ty_ref) = prefix_type_ref(type_path) else {
            return Ok(None);
        };
        let prefix_ty = self.ty_from_type_ref_in_scope_with_subst(&prefix_ty_ref, scope, subst)?;

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
        args: &[IndexedGenericArg],
        subst: &IndexedTypeSubst,
    ) -> Result<IndexedTy, PackageStoreError> {
        if aliases.len() != 1 {
            return Ok(IndexedTy::Unknown);
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
        args: &[IndexedGenericArg],
        subst: &IndexedTypeSubst,
    ) -> Result<IndexedTy, PackageStoreError> {
        let item_query = self.item_query();
        let Some(alias_data) = item_query.type_alias_data(alias_ref)? else {
            return Ok(IndexedTy::Unknown);
        };
        let Some(aliased_ty) = alias_data.signature.aliased_ty() else {
            return Ok(IndexedTy::Unknown);
        };
        if type_ref_is_self(aliased_ty) {
            return Ok(IndexedTy::Unknown);
        }

        let mut alias_subst = subst.clone();
        if let Some(generics) = alias_data.signature.generics() {
            alias_subst.extend(subst_from_generics(generics, args));
        }

        let context = item_query
            .type_path_context_for_owner(alias_ref.origin, alias_data.owner)?
            .unwrap_or_else(|| TypePathContext::module(self.body.owner_module));
        if context.module.origin == DefMapRef::Body(self.body_ref) {
            self.ty_from_type_ref_in_module_with_subst(aliased_ty, context.module, &alias_subst)
        } else {
            self.ty_from_type_ref_in_context_with_subst(aliased_ty, context, &alias_subst)
        }
    }

    fn associated_type_alias_for_type(
        &self,
        ty: &IndexedNominalTy,
        name: &str,
    ) -> Result<Option<TypeAliasRef>, PackageStoreError> {
        if ty.def.origin != DefMapRef::Body(self.body_ref) {
            return Ok(None);
        }

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
        receiver_ty: &IndexedNominalTy,
        args: &[IndexedGenericArg],
    ) -> Result<IndexedTy, PackageStoreError> {
        let item_query = self.item_query();
        let Some(alias_data) = item_query.type_alias_data(alias_ref)? else {
            return Ok(IndexedTy::Unknown);
        };
        let Some(aliased_ty) = alias_data.signature.aliased_ty() else {
            return Ok(IndexedTy::Unknown);
        };
        if type_ref_is_self(aliased_ty) {
            return Ok(IndexedTyRepr::nominal(vec![receiver_ty.clone()]));
        }

        let mut alias_subst = self.semantic_type_subst(receiver_ty)?;
        if let ItemOwner::Impl(impl_id) = alias_data.owner {
            let impl_ref = ImplRef {
                origin: alias_ref.origin,
                id: impl_id,
            };
            if let Some(impl_data) = item_query.impl_data(impl_ref)? {
                alias_subst.extend(
                    self.semantic_impl_matcher()
                        .impl_self_subst_for_impl(impl_data, receiver_ty),
                );
            }
        }
        if let Some(generics) = alias_data.signature.generics() {
            alias_subst.extend(subst_from_generics(generics, args));
        }

        let context = item_query
            .type_path_context_for_owner(alias_ref.origin, alias_data.owner)?
            .unwrap_or_else(|| TypePathContext::module(self.body.owner_module));
        if context.module.origin == DefMapRef::Body(self.body_ref) {
            self.ty_from_type_ref_in_module_with_subst(aliased_ty, context.module, &alias_subst)
        } else {
            self.ty_from_type_ref_in_context_with_subst(aliased_ty, context, &alias_subst)
        }
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

    fn generic_args_from_type_path_in_scope(
        &self,
        type_path: &rg_item_tree::TypePath,
        scope: ScopeId,
        subst: &IndexedTypeSubst,
    ) -> Result<Vec<IndexedGenericArg>, PackageStoreError> {
        let Some(segment) = type_path.segments.last() else {
            return Ok(Vec::new());
        };
        self.generic_args_from_item_tree_args_in_scope(&segment.args, scope, subst)
    }

    fn generic_args_from_item_tree_args_in_scope(
        &self,
        args: &[GenericArg],
        scope: ScopeId,
        subst: &IndexedTypeSubst,
    ) -> Result<Vec<IndexedGenericArg>, PackageStoreError> {
        let mut generic_args = Vec::new();
        for arg in args {
            generic_args.push(self.generic_arg_from_item_tree_arg_in_scope(arg, scope, subst)?);
        }
        Ok(generic_args)
    }

    fn generic_arg_from_item_tree_arg_in_scope(
        &self,
        arg: &GenericArg,
        scope: ScopeId,
        subst: &IndexedTypeSubst,
    ) -> Result<IndexedGenericArg, PackageStoreError> {
        match arg {
            GenericArg::Type(ty) => Ok(IndexedGenericArg::Type(Box::new(
                self.ty_from_type_ref_in_scope_with_subst(ty, scope, subst)?,
            ))),
            GenericArg::Lifetime(lifetime) => Ok(IndexedGenericArg::Lifetime(lifetime.clone())),
            GenericArg::Const(value) => Ok(IndexedGenericArg::Const(value.clone())),
            GenericArg::AssocType { name, ty } => Ok(IndexedGenericArg::AssocType {
                name: name.clone(),
                ty: match ty {
                    Some(ty) => Some(Box::new(
                        self.ty_from_type_ref_in_scope_with_subst(ty, scope, subst)?,
                    )),
                    None => None,
                },
            }),
            GenericArg::Unsupported(text) => Ok(IndexedGenericArg::Unsupported(text.clone())),
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
