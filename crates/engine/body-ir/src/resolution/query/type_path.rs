//! Type-path lookup.

use rg_ir_model::{
    DefId, DefMapRef, ModuleId, ModuleRef, Path, ScopeId, SemanticItemRef, TypePathResolution,
};
use rg_ir_storage::{DefMapSource, ItemStoreSource, NameResolutionFilter, TypePathContext};
use rg_package_store::PackageStoreError;
use rg_std::{ExpectedUnique, UniqueVec};
use rg_ty::Ty;

use crate::resolution::BodyResolutionContext;

/// Resolves paths in the type namespace.
///
/// Handles body scopes, body-local modules, and owner contexts.
pub struct BodyTypePathQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyTypePathQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Resolve a path such as `Self` or `foo::bar` within a body scope.
    pub fn resolve_in_scope(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<TypePathResolution, PackageStoreError> {
        if let Some((prefix, name)) = path.split_prefix_name() {
            let prefix_resolution = self.resolve_in_scope(scope, &prefix)?;
            let prefix_ty =
                Ty::from_type_path_resolution(prefix_resolution, Vec::new()).unwrap_or(Ty::Unknown);
            let mut aliases = ExpectedUnique::new();
            for ty in prefix_ty.as_nominals() {
                if let Some(alias) = self
                    .context
                    .type_aliases()
                    .associated_alias_for_type(ty, name)?
                {
                    aliases.push(alias);
                }
            }
            if !aliases.is_empty() {
                return Ok(TypePathResolution::type_alias(aliases));
            }
        }

        let body_items = self.resolve_body_type_items_from_def_map(scope, path)?;
        if !body_items.is_empty() {
            return Ok(self.type_resolution_from_items(body_items));
        }

        let item_paths = self.context.item_paths();
        let context = self.context.type_contexts().for_body_owner()?;
        let resolution = item_paths.resolve_type_path(context, path)?;
        if !matches!(resolution, TypePathResolution::Unknown) {
            return Ok(resolution);
        }

        let fallback_module = self.context.body().fallback_module();
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

    /// Resolve a path such as `Self` or `foo::bar` within an owner context.
    pub(crate) fn resolve_in_context(
        &self,
        context: TypePathContext,
        path: &Path,
    ) -> Result<TypePathResolution, PackageStoreError> {
        if !matches!(context.module.origin, DefMapRef::Body(_)) {
            return self.context.item_paths().resolve_type_path(context, path);
        }

        if path.is_self_type() {
            let candidate = self
                .context
                .type_contexts()
                .nominal_self_ty_for_context(context)?
                .map(|ty| ty.def);
            return Ok(TypePathResolution::self_type(candidate));
        }

        if let Some((prefix, name)) = path.split_prefix_name() {
            let prefix_resolution = self.resolve_in_context(context, &prefix)?;
            let prefix_ty =
                Ty::from_type_path_resolution(prefix_resolution, Vec::new()).unwrap_or(Ty::Unknown);
            let mut aliases = ExpectedUnique::new();
            for ty in prefix_ty.as_nominals() {
                if let Some(alias) = self
                    .context
                    .type_aliases()
                    .associated_alias_for_type(ty, name)?
                {
                    aliases.push(alias);
                }
            }
            if !aliases.is_empty() {
                return Ok(TypePathResolution::type_alias(aliases));
            }
        }

        let body_items = self.resolve_body_type_items_from_module(context.module, path)?;
        Ok(self.type_resolution_from_items(body_items))
    }

    /// Resolve a lexical body type path from a scope.
    fn resolve_body_type_items_from_def_map(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<UniqueVec<SemanticItemRef>, PackageStoreError> {
        let from = ModuleRef {
            origin: DefMapRef::Body(self.context.body_ref()),
            module: ModuleId(scope.0),
        };
        let result = self
            .context
            .def_map_query()
            .scope_resolver()
            .resolve_lexical_path(from, path, NameResolutionFilter::TypesOnly)?;

        self.semantic_items_for_defs(result.resolved)
    }

    /// Resolve a body-module type path, then try the fallback module.
    fn resolve_body_type_items_from_module(
        &self,
        module: ModuleRef,
        path: &Path,
    ) -> Result<UniqueVec<SemanticItemRef>, PackageStoreError> {
        let def_maps = self.context.def_map_query();
        let result = def_maps.scope_resolver().resolve_path(
            module,
            path,
            NameResolutionFilter::TypesOnly,
        )?;
        let items = self.semantic_items_for_defs(result.resolved)?;
        if !items.is_empty() {
            return Ok(items);
        }

        // A body-local module only carries the lexical body facts. The inherited fallback keeps
        // signatures on parent body-local items able to name ordinary surrounding module items.
        let fallback_module = self.context.body().fallback_module();
        if fallback_module == module {
            return Ok(items);
        }

        let result = def_maps.scope_resolver().resolve_path(
            fallback_module,
            path,
            NameResolutionFilter::TypesOnly,
        )?;
        self.semantic_items_for_defs(result.resolved)
    }

    /// Convert local defs into semantic item refs.
    fn semantic_items_for_defs(
        &self,
        defs: Vec<DefId>,
    ) -> Result<UniqueVec<SemanticItemRef>, PackageStoreError> {
        let mut items = UniqueVec::new();
        for def in defs {
            let DefId::Local(local_def) = def else {
                continue;
            };
            if let Some(item) = self
                .context
                .item_query()
                .semantic_item_for_local_def(local_def)?
            {
                items.push(item);
            }
        }
        Ok(items)
    }

    /// Group semantic items into a type-namespace resolution.
    fn type_resolution_from_items(&self, items: UniqueVec<SemanticItemRef>) -> TypePathResolution {
        let mut type_defs = ExpectedUnique::new();
        let mut type_aliases = ExpectedUnique::new();
        let mut traits = ExpectedUnique::new();
        for item in items {
            match item {
                SemanticItemRef::TypeDef(type_def) => {
                    type_defs.push(type_def);
                }
                SemanticItemRef::TypeAlias(type_alias) => {
                    type_aliases.push(type_alias);
                }
                SemanticItemRef::Trait(trait_ref) => {
                    traits.push(trait_ref);
                }
                SemanticItemRef::Impl(_)
                | SemanticItemRef::Function(_)
                | SemanticItemRef::Const(_)
                | SemanticItemRef::Static(_) => {}
            }
        }

        if !type_defs.is_empty() {
            TypePathResolution::type_def(type_defs)
        } else if !type_aliases.is_empty() {
            TypePathResolution::type_alias(type_aliases)
        } else if !traits.is_empty() {
            TypePathResolution::trait_ref(traits)
        } else {
            TypePathResolution::Unknown
        }
    }
}
