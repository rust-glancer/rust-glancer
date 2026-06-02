//! Shared path-to-item queries over DefMap and item-store providers.
//!
//! DefMap lookup answers "which definitions does this path name?", while the item store
//! answers "which semantic item does this local definition lower to?". This query keeps that
//! composition out of storage transactions.

use rg_def_map::{DefMapQuery, DefMapSource, Path};
use rg_ir_model::{DefId, ModuleRef, SemanticItemRef, TraitRef, TypeDefRef, TypePathResolution};
use rg_package_store::PackageStoreError;

use crate::{ItemStoreQuery, ItemStoreSource, TypePathContext, push_unique};

/// Resolves paths into semantic-shaped item refs using independent DefMap and ItemStore sources.
#[derive(Clone)]
pub struct ItemPathQuery<'a, D, I> {
    def_maps: DefMapQuery<D>,
    items: ItemStoreQuery<'a, I>,
}

impl<'a, D, I> ItemPathQuery<'a, D, I>
where
    D: DefMapSource<Error = PackageStoreError>,
    I: ItemStoreSource<'a, Error = PackageStoreError>,
{
    pub fn new(def_maps: D, items: I) -> Self {
        Self {
            def_maps: DefMapQuery::new(def_maps),
            items: ItemStoreQuery::new(items),
        }
    }

    /// Gives algorithms access to item data after path resolution has selected semantic refs.
    pub fn items(&self) -> &ItemStoreQuery<'a, I> {
        &self.items
    }

    /// Resolves a type-position path into the type resolution shape used by type projection.
    pub fn resolve_type_path(
        &self,
        context: TypePathContext,
        path: &Path,
    ) -> Result<TypePathResolution, PackageStoreError> {
        if path.is_self_type() {
            let Some(impl_ref) = context.impl_ref else {
                return Ok(TypePathResolution::Unknown);
            };
            let types = self
                .items
                .impl_data(impl_ref)?
                .map(|data| data.resolved_self_tys.clone())
                .unwrap_or_default();
            return Ok(if types.is_empty() {
                TypePathResolution::Unknown
            } else {
                TypePathResolution::SelfType(types)
            });
        }

        let type_defs = self.type_defs_for_path(context.module, path)?;
        if type_defs.is_empty() {
            let traits = self.traits_for_path(context.module, path)?;
            Ok(if traits.is_empty() {
                TypePathResolution::Unknown
            } else {
                TypePathResolution::Traits(traits)
            })
        } else {
            Ok(TypePathResolution::TypeDefs(type_defs))
        }
    }

    /// Resolves a type-position path into canonical item refs, preserving `Self` handling.
    pub fn semantic_items_for_type_path(
        &self,
        context: TypePathContext,
        path: &Path,
    ) -> Result<Vec<SemanticItemRef>, PackageStoreError> {
        if path.is_self_type() {
            if let Some(impl_ref) = context.impl_ref
                && let Some(data) = self.items.impl_data(impl_ref)?
            {
                let items = data
                    .resolved_self_tys
                    .iter()
                    .copied()
                    .map(SemanticItemRef::from)
                    .collect();
                return Ok(items);
            } else {
                return Ok(Vec::new());
            };
        }

        self.semantic_items_for_path(context.module, path)
    }

    /// Filters a type-position path to nominal type definitions.
    pub fn type_defs_for_path(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> Result<Vec<TypeDefRef>, PackageStoreError> {
        Ok(self
            .semantic_items_for_path(from, path)?
            .into_iter()
            .filter_map(|item| match item {
                SemanticItemRef::TypeDef(ty) => Some(ty),
                _ => None,
            })
            .collect())
    }

    /// Filters a type-position path to trait definitions.
    pub fn traits_for_path(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> Result<Vec<TraitRef>, PackageStoreError> {
        Ok(self
            .semantic_items_for_path(from, path)?
            .into_iter()
            .filter_map(|item| match item {
                SemanticItemRef::Trait(trait_ref) => Some(trait_ref),
                _ => None,
            })
            .collect())
    }

    /// Resolves through the type namespace and projects local definitions into item refs.
    fn semantic_items_for_path(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> Result<Vec<SemanticItemRef>, PackageStoreError> {
        let result = self.def_maps.resolve_path_in_type_namespace(from, path)?;
        let mut resolved_items = Vec::new();
        for def in result.resolved {
            if let DefId::Local(local_def) = def
                && let Some(item) = self.items.semantic_item_for_local_def(local_def)?
            {
                push_unique(&mut resolved_items, item);
            }
        }

        Ok(resolved_items)
    }
}
