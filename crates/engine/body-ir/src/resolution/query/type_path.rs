//! Type-path lookup inside a lowered body.
//!
//! A body has lexical DefMap scopes for local declarations, but it also inherits the module and impl
//! context of its owner. This query keeps that lookup order in one place and handles associated type
//! aliases, which are not ordinary entries in either scope graph.

use rg_def_map::{DefMapSource, NamespaceSet};
use rg_ir_model::{DefId, DefMapRef, EnumVariantRef, ModuleId, ModuleRef, Path, ScopeId};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemStoreSource, TypePathContext, TypePathResolution};
use rg_std::ExpectedUnique;
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

    /// Resolves a path such as `Self` or `foo::bar` within a body scope.
    ///
    /// Associated aliases are checked first. Ordinary names then search the lexical body scopes, the
    /// owner's module/impl context, and finally the surrounding module inherited by a body-local item.
    pub fn resolve_in_scope(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<TypePathResolution, PackageStoreError> {
        // `Type::Alias` is selected from impls rather than a DefMap module entry. Resolve it before
        // applying the ordinary lexical/module lookup order below.
        if let Some((prefix, name)) = path.split_prefix_name() {
            let prefix_resolution = self.resolve_in_scope(scope, &prefix)?;
            let prefix_ty =
                Ty::from_type_path_resolution(prefix_resolution, Vec::new()).unwrap_or(Ty::Unknown);
            let mut aliases = ExpectedUnique::new();
            for ty in prefix_ty.as_adts() {
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

        // A declaration such as `struct Local;` inside the body shadows names inherited from the
        // owner, so the synthetic body module is the first ordinary lookup location.
        let from = ModuleRef {
            origin: DefMapRef::Body(self.context.body_ref()),
            module: ModuleId(scope.0),
        };
        let lexical_resolution = self
            .context
            .item_paths()
            .resolve_lexical_type_path(from, path)?;
        if !matches!(lexical_resolution, TypePathResolution::Unknown) {
            return Ok(lexical_resolution);
        }

        // Names absent from the body may still come from the item's module or impl, including `Self`.
        let item_paths = self.context.item_paths();
        let context = self.context.type_contexts().for_body_owner()?;
        let resolution = item_paths.resolve_type_path(context, path)?;
        if !matches!(resolution, TypePathResolution::Unknown) {
            return Ok(resolution);
        }

        // A body-local owner can itself live in a synthetic module. Its fallback points back to the
        // ordinary surrounding module where imports and sibling items are declared.
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

    /// Resolve an enum variant selected through the type namespace.
    ///
    /// All variants have a type-namespace binding, but they are not themselves Rust types and
    /// therefore cannot be represented by `TypePathResolution`. Record syntax such as
    /// `Choice::Record { value: 1 }` asks for the variant identity through this separate path. The
    /// lookup order still mirrors [`Self::resolve_in_scope`]: lexical body, owner, then fallback.
    pub fn resolve_enum_variant_in_scope(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<Option<EnumVariantRef>, PackageStoreError> {
        let from = ModuleRef {
            origin: DefMapRef::Body(self.context.body_ref()),
            module: ModuleId(scope.0),
        };
        let def_maps = self.context.def_map_query();
        let result =
            def_maps
                .scope_resolver()
                .resolve_lexical_path(from, path, NamespaceSet::TYPES)?;
        if !result.resolved.is_empty() {
            return self.enum_variant_from_defs(result.resolved);
        }

        let owner_module = self.context.body().owner_module();
        let result =
            def_maps
                .scope_resolver()
                .resolve_path(owner_module, path, NamespaceSet::TYPES)?;
        if !result.resolved.is_empty() {
            return self.enum_variant_from_defs(result.resolved);
        }

        let fallback_module = self.context.body().fallback_module();
        if fallback_module == owner_module {
            return Ok(None);
        }
        let result =
            def_maps
                .scope_resolver()
                .resolve_path(fallback_module, path, NamespaceSet::TYPES)?;
        self.enum_variant_from_defs(result.resolved)
    }

    /// Resolve a path such as `Self` or `foo::bar` within an owner context.
    pub(crate) fn resolve_in_context(
        &self,
        context: TypePathContext,
        path: &Path,
    ) -> Result<TypePathResolution, PackageStoreError> {
        if path.is_self_type() {
            let Some(impl_ref) = context.impl_ref else {
                return Ok(TypePathResolution::Unknown);
            };
            let Some(impl_data) = self.context.item_query().impl_data(impl_ref)? else {
                return Ok(TypePathResolution::Unknown);
            };

            // Keep path resolution at the identity layer. Impl-header lowering uses this resolver
            // for clauses such as `where Self: Trait`; asking for the full header here would make
            // resolving that `Self` depend recursively on the header being built.
            return Ok(TypePathResolution::self_type(
                impl_data.resolved_self_ty.clone(),
            ));
        }

        // Associated aliases are not ordinary module-scope path items, so handle `Type::Alias`
        // before the normal body/semantic item lookup.
        if let Some((prefix, name)) = path.split_prefix_name() {
            let prefix_resolution = self.resolve_in_context(context, &prefix)?;
            let prefix_ty =
                Ty::from_type_path_resolution(prefix_resolution, Vec::new()).unwrap_or(Ty::Unknown);
            let mut aliases = ExpectedUnique::new();
            for ty in prefix_ty.as_adts() {
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

        let item_paths = self.context.item_paths();
        let resolution = item_paths.resolve_type_path(context, path)?;
        if !matches!(context.module.origin, DefMapRef::Body(_))
            || !matches!(resolution, TypePathResolution::Unknown)
        {
            return Ok(resolution);
        }

        // A body-local module only carries the lexical body facts. The inherited fallback keeps
        // signatures on parent body-local items able to name ordinary surrounding module items.
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

    fn enum_variant_from_defs(
        &self,
        defs: Vec<DefId>,
    ) -> Result<Option<EnumVariantRef>, PackageStoreError> {
        let source = self.context.def_map_source();
        let item_query = self.context.item_query();
        let mut variants = ExpectedUnique::new();
        for def in defs {
            let DefId::EnumVariant(variant_ref) = def else {
                continue;
            };
            let Some(variant_data) = source.local_enum_variant_data(variant_ref)? else {
                continue;
            };
            if let Some(variant_ref) =
                item_query.enum_variant_ref_for_local_enum_variant(variant_ref, variant_data)?
            {
                variants.push(variant_ref);
            }
        }
        Ok(variants.into_option())
    }
}
