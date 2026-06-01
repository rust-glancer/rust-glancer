//! Read transactions over frozen Semantic IR package data.

use rg_def_map::{DefMapReadTxn, PackageSlot, Path};
use rg_ir_model::{DefId, ModuleRef, TargetRef};
use rg_ir_model::{DefMapRef, SemanticItemRef, TraitRef, TypeDefRef};
use rg_package_store::{PackageStoreError, PackageStoreReadTxn};

use rg_ir_model::TypePathResolution;

use crate::{ItemStore, ItemStoreQuery, ItemStoreSource, PackageIr, TypePathContext, push_unique};

/// Read-only semantic IR access for one query transaction.
#[derive(Debug, Clone)]
pub struct SemanticIrReadTxn<'db> {
    packages: PackageStoreReadTxn<'db, PackageIr>,
}

impl<'db> SemanticIrReadTxn<'db> {
    pub(crate) fn from_package_store(packages: PackageStoreReadTxn<'db, PackageIr>) -> Self {
        Self { packages }
    }

    pub fn package(&self, package: PackageSlot) -> Result<&PackageIr, PackageStoreError> {
        self.packages.read(package)
    }

    pub fn items(&self, target: TargetRef) -> Result<Option<&ItemStore>, PackageStoreError> {
        let package = self.package(target.package)?;
        Ok(package.target(target.target))
    }

    pub fn included_stores(&self) -> Result<Vec<&ItemStore>, PackageStoreError> {
        let mut target_stores = Vec::new();

        for package in self.packages.included_packages() {
            target_stores.extend(package?.targets().iter())
        }
        Ok(target_stores)
    }

    pub fn resolve_type_path(
        &self,
        def_map: &DefMapReadTxn<'db>,
        context: TypePathContext,
        path: &Path,
    ) -> Result<TypePathResolution, PackageStoreError> {
        if path.is_self_type() {
            let Some(impl_ref) = context.impl_ref else {
                return Ok(TypePathResolution::Unknown);
            };
            let types = ItemStoreQuery::new(self)
                .impl_data(impl_ref)?
                .map(|data| data.resolved_self_tys.clone())
                .unwrap_or_default();
            return Ok(if types.is_empty() {
                TypePathResolution::Unknown
            } else {
                TypePathResolution::SelfType(types)
            });
        }

        let type_defs = self.type_defs_for_path(def_map, context.module, path)?;
        if type_defs.is_empty() {
            let traits = self.traits_for_path(def_map, context.module, path)?;
            Ok(if traits.is_empty() {
                TypePathResolution::Unknown
            } else {
                TypePathResolution::Traits(traits)
            })
        } else {
            Ok(TypePathResolution::TypeDefs(type_defs))
        }
    }

    pub fn semantic_items_for_path(
        &self,
        def_map: &DefMapReadTxn<'db>,
        from: ModuleRef,
        path: &Path,
    ) -> Result<Vec<SemanticItemRef>, PackageStoreError> {
        self.resolve_path(def_map, from, path, |db, def| {
            let DefId::Local(local_def) = def else {
                return Ok(None);
            };
            ItemStoreQuery::new(db).semantic_item_for_local_def(local_def)
        })
    }

    pub fn semantic_items_for_type_path(
        &self,
        def_map: &DefMapReadTxn<'db>,
        context: TypePathContext,
        path: &Path,
    ) -> Result<Vec<SemanticItemRef>, PackageStoreError> {
        if path.is_self_type() {
            if let Some(impl_ref) = context.impl_ref
                && let Some(data) = ItemStoreQuery::new(self).impl_data(impl_ref)?
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

        self.semantic_items_for_path(def_map, context.module, path)
    }

    pub fn type_defs_for_path(
        &self,
        def_map: &DefMapReadTxn<'db>,
        from: ModuleRef,
        path: &Path,
    ) -> Result<Vec<TypeDefRef>, PackageStoreError> {
        Ok(self
            .semantic_items_for_path(def_map, from, path)?
            .into_iter()
            .filter_map(|item| match item {
                SemanticItemRef::TypeDef(ty) => Some(ty),
                _ => None,
            })
            .collect())
    }

    pub fn traits_for_path(
        &self,
        def_map: &DefMapReadTxn<'db>,
        from: ModuleRef,
        path: &Path,
    ) -> Result<Vec<TraitRef>, PackageStoreError> {
        Ok(self
            .semantic_items_for_path(def_map, from, path)?
            .into_iter()
            .filter_map(|item| match item {
                SemanticItemRef::Trait(trait_ref) => Some(trait_ref),
                _ => None,
            })
            .collect())
    }

    fn resolve_path<T: PartialEq>(
        &self,
        def_map: &DefMapReadTxn<'db>,
        owner: ModuleRef,
        path: &Path,
        map_def: impl Fn(&Self, DefId) -> Result<Option<T>, PackageStoreError>,
    ) -> Result<Vec<T>, PackageStoreError> {
        let mut resolved_items = Vec::new();
        let result = def_map.resolve_path_in_type_namespace(owner, path)?;
        for def in result.resolved {
            let Some(item) = map_def(self, def)? else {
                continue;
            };
            push_unique(&mut resolved_items, item);
        }

        Ok(resolved_items)
    }
}

impl<'a, 'db> ItemStoreSource<'a> for &'a SemanticIrReadTxn<'db> {
    type Error = PackageStoreError;

    fn item_store_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<&'a ItemStore>, Self::Error> {
        let Some(target) = origin.as_target_ref() else {
            return Ok(None);
        };

        (*self).items(target)
    }

    fn visible_stores(&self) -> Result<Vec<&'a ItemStore>, Self::Error> {
        (*self).included_stores()
    }
}
