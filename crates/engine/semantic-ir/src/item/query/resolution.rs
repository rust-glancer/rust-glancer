//! Definition-level path resolution over def maps and semantic item stores.
//!
//! This query stops at semantic item identities. Type projection can consume those identities,
//! but definition lowering and impl-header resolution do not need to depend on the type engine.

use rg_def_map::{DefMapQuery, DefMapSource, NamespaceSet};
use rg_ir_model::{DefId, ModuleRef, Path, SemanticItemRef, TraitDefRef, TypeDefRef};
use rg_std::{ExpectedUnique, UniqueVec};

use super::{ItemStoreQuery, ItemStoreSource};
use crate::{TypePathContext, TypePathResolution};

/// Resolves Rust paths into semantic item identities without projecting them into `Ty`.
#[derive(Clone)]
pub struct ItemResolutionQuery<'a, D, I> {
    def_maps: DefMapQuery<D>,
    items: ItemStoreQuery<'a, I>,
}

impl<'a, D, I> ItemResolutionQuery<'a, D, I>
where
    D: DefMapSource,
    I: ItemStoreSource<'a, Error = D::Error>,
{
    pub fn new(def_maps: D, items: I) -> Self {
        Self {
            def_maps: DefMapQuery::new(def_maps),
            items: ItemStoreQuery::new(items),
        }
    }

    pub fn items(&self) -> &ItemStoreQuery<'a, I> {
        &self.items
    }

    /// Resolves a type-position path into semantic type or trait identities.
    pub fn resolve_type_path(
        &self,
        context: TypePathContext,
        path: &Path,
    ) -> Result<TypePathResolution, D::Error> {
        if path.is_self_type() {
            let Some(impl_ref) = context.impl_ref else {
                return Ok(TypePathResolution::Unknown);
            };
            if let Some(data) = self.items.impl_data(impl_ref)? {
                return Ok(TypePathResolution::self_type(data.resolved_self_ty.clone()));
            }
            return Ok(TypePathResolution::Unknown);
        }

        Ok(Self::type_resolution_from_items(
            self.semantic_items_for_path(context.module, path)?,
        ))
    }

    /// Resolves a relative type path through body lexical scopes only.
    ///
    /// For `fn f() { struct Local; let _: Local; }`, `from` names the synthetic DefMap module for
    /// the `let` scope. The resolved local definition is projected into the same semantic identity
    /// returned by module path resolution; owner-module and `Self` fallbacks remain the caller's job.
    pub fn resolve_lexical_type_path(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> Result<TypePathResolution, D::Error> {
        let result =
            self.def_maps
                .scope_resolver()
                .resolve_lexical_path(from, path, NamespaceSet::TYPES)?;
        Ok(Self::type_resolution_from_items(
            self.semantic_items_for_defs(result.resolved)?,
        ))
    }

    /// Resolves a type-position path into item refs while preserving `Self` handling.
    pub fn semantic_items_for_type_path(
        &self,
        context: TypePathContext,
        path: &Path,
    ) -> Result<UniqueVec<SemanticItemRef>, D::Error> {
        if path.is_self_type() {
            if let Some(impl_ref) = context.impl_ref
                && let Some(data) = self.items.impl_data(impl_ref)?
                && let Some(ty) = data.resolved_self_ty.as_option()
            {
                return Ok([SemanticItemRef::from(*ty)].into_iter().collect());
            }
            return Ok(UniqueVec::new());
        }

        self.semantic_items_for_path(context.module, path)
    }

    pub fn type_defs_for_path(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> Result<UniqueVec<TypeDefRef>, D::Error> {
        Ok(self
            .semantic_items_for_path(from, path)?
            .into_iter()
            .filter_map(|item| match item {
                SemanticItemRef::TypeDef(ty) => Some(ty),
                _ => None,
            })
            .collect())
    }

    pub fn traits_for_path(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> Result<UniqueVec<TraitDefRef>, D::Error> {
        Ok(self
            .semantic_items_for_path(from, path)?
            .into_iter()
            .filter_map(|item| match item {
                SemanticItemRef::Trait(trait_ref) => Some(trait_ref),
                _ => None,
            })
            .collect())
    }

    /// Resolves through the type namespace and projects def-map locals into semantic refs.
    fn semantic_items_for_path(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> Result<UniqueVec<SemanticItemRef>, D::Error> {
        let result =
            self.def_maps
                .scope_resolver()
                .resolve_path(from, path, NamespaceSet::TYPES)?;
        self.semantic_items_for_defs(result.resolved)
    }

    /// Projects DefMap-local definitions that have a declaration in the Semantic IR item store.
    ///
    /// Other resolved identities, such as enum variants, are intentionally handled by their
    /// dedicated query because they are not standalone Rust types.
    fn semantic_items_for_defs(
        &self,
        defs: Vec<DefId>,
    ) -> Result<UniqueVec<SemanticItemRef>, D::Error> {
        let mut resolved_items = UniqueVec::new();
        for def in defs {
            if let DefId::Local(local_def) = def
                && let Some(item) = self.items.semantic_item_for_local_def(local_def)?
            {
                resolved_items.push(item);
            }
        }

        Ok(resolved_items)
    }

    /// Groups resolved identities by the precedence used for a type-position path.
    ///
    /// Concrete type definitions win over aliases, and aliases win over traits. Aliases remain
    /// identities here even though the type layer later expands them; resolving them early would
    /// lose the alias's written generic arguments.
    fn type_resolution_from_items(items: UniqueVec<SemanticItemRef>) -> TypePathResolution {
        let mut type_defs = ExpectedUnique::new();
        let mut type_aliases = ExpectedUnique::new();
        let mut traits = ExpectedUnique::new();
        for item in items {
            match item {
                SemanticItemRef::TypeDef(type_def) => type_defs.push(type_def),
                SemanticItemRef::TypeAlias(alias) => type_aliases.push(alias),
                SemanticItemRef::Trait(trait_ref) => traits.push(trait_ref),
                SemanticItemRef::Impl(_)
                | SemanticItemRef::Function(_)
                | SemanticItemRef::Const(_)
                | SemanticItemRef::Static(_) => {}
            }
        }
        if !type_defs.is_empty() {
            return TypePathResolution::type_def(type_defs);
        }
        if !type_aliases.is_empty() {
            return TypePathResolution::type_alias(type_aliases);
        }
        TypePathResolution::trait_ref(traits)
    }
}
