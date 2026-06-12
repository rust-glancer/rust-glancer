//! Canonical path projection for indexed declarations.
//!
//! This view follows DefMap module parents and Semantic IR owners to produce the stable Rust-ish
//! paths used by hover, completion details, and symbol containers. It intentionally does not try to
//! reconstruct import aliases or rustdoc-style canonicalization.

use rg_ir_model::{
    ConstRef, DefMapRef, FunctionRef, ImplId, ImplRef, ItemOwner, ModuleRef, StaticRef, TraitRef,
    TypeAliasRef, TypeDefId, TypeDefRef, hir::items::EnumVariantData,
};
use rg_ir_storage::{DefMapQuery, ItemStoreQuery};

use crate::IndexedViewDb;

pub struct PathView<'a, 'db>(&'a IndexedViewDb<'db>);

impl<'a, 'db> PathView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self(db)
    }

    pub fn module_path(&self, module_ref: ModuleRef) -> anyhow::Result<Option<String>> {
        let Some(target) = module_ref.origin.as_target_ref() else {
            return Ok(None);
        };
        let package = self.0.def_map.package(target.package)?;
        let def_maps = DefMapQuery::new(self.0);
        let mut names = Vec::new();
        let mut current = module_ref.module;

        // Module ids form a parent chain rooted at the target module. Walking it upward and then
        // reversing gives us the same crate::item::module::child shape users see in Rust paths.
        loop {
            let Some(module) = def_maps.module_data(ModuleRef {
                origin: module_ref.origin,
                module: current,
            })?
            else {
                return Ok(None);
            };
            if let Some(name) = &module.name {
                names.push(name.to_string());
            }

            let Some(parent) = module.parent else {
                break;
            };
            current = parent;
        }

        names.push(
            package
                .target_name(target.target)
                .unwrap_or_else(|| package.package_name())
                .to_string(),
        );
        names.reverse();
        Ok(Some(names.join("::")))
    }

    pub fn type_def_path(&self, ty: TypeDefRef) -> anyhow::Result<Option<String>> {
        let Some((module, name)) = self.type_def_owner_and_name(ty)? else {
            return Ok(None);
        };
        self.path_in_module(module, name)
    }

    pub fn trait_path(&self, trait_ref: TraitRef) -> anyhow::Result<Option<String>> {
        let Some(data) = ItemStoreQuery::new(self.0).trait_data(trait_ref)? else {
            return Ok(None);
        };
        self.path_in_module(data.owner, &data.name)
    }

    pub fn function_path(&self, function_ref: FunctionRef) -> anyhow::Result<Option<String>> {
        let Some(data) = ItemStoreQuery::new(self.0).function_data(function_ref)? else {
            return Ok(None);
        };
        Ok(self
            .path_for_owner(function_ref.origin, data.owner)?
            .map(|owner| format!("{owner}::{}", data.name)))
    }

    pub fn type_alias_path(&self, type_alias_ref: TypeAliasRef) -> anyhow::Result<Option<String>> {
        let Some(data) = ItemStoreQuery::new(self.0).type_alias_data(type_alias_ref)? else {
            return Ok(None);
        };
        Ok(self
            .path_for_owner(type_alias_ref.origin, data.owner)?
            .map(|owner| format!("{owner}::{}", data.name)))
    }

    pub fn const_path(&self, const_ref: ConstRef) -> anyhow::Result<Option<String>> {
        let Some(data) = ItemStoreQuery::new(self.0).const_data(const_ref)? else {
            return Ok(None);
        };
        Ok(self
            .path_for_owner(const_ref.origin, data.owner)?
            .map(|owner| format!("{owner}::{}", data.name)))
    }

    pub fn static_path(&self, static_ref: StaticRef) -> anyhow::Result<Option<String>> {
        let Some(data) = ItemStoreQuery::new(self.0).static_data(static_ref)? else {
            return Ok(None);
        };
        self.path_in_module(data.owner, &data.name)
    }

    pub fn enum_variant_path(&self, data: EnumVariantData<'_>) -> anyhow::Result<Option<String>> {
        Ok(self
            .type_def_path(data.owner)?
            .map(|owner| format!("{owner}::{}", data.variant.name)))
    }

    pub fn path_in_module(
        &self,
        module_ref: ModuleRef,
        name: &str,
    ) -> anyhow::Result<Option<String>> {
        Ok(self
            .module_path(module_ref)?
            .map(|module| format!("{module}::{name}")))
    }

    fn path_for_owner(
        &self,
        origin: DefMapRef,
        owner: ItemOwner,
    ) -> anyhow::Result<Option<String>> {
        match owner {
            ItemOwner::Module(module_ref) => self.module_path(module_ref),
            ItemOwner::Trait(trait_id) => self.trait_path(TraitRef {
                origin,
                id: trait_id,
            }),
            ItemOwner::Impl(impl_id) => self.impl_self_path(origin, impl_id),
        }
    }

    fn impl_self_path(&self, origin: DefMapRef, impl_id: ImplId) -> anyhow::Result<Option<String>> {
        let Some(data) = ItemStoreQuery::new(self.0).impl_data(ImplRef {
            origin,
            id: impl_id,
        })?
        else {
            return Ok(None);
        };

        if let Some(ty) = data.resolved_self_ty.as_option()
            && let Some(path) = self.type_def_path(*ty)?
        {
            return Ok(Some(path));
        }

        self.module_path(data.owner)
    }

    fn type_def_owner_and_name(&self, ty: TypeDefRef) -> anyhow::Result<Option<(ModuleRef, &str)>> {
        let item_query = ItemStoreQuery::new(self.0);
        let Some(items) = item_query.item_store_for_origin(ty.origin)? else {
            return Ok(None);
        };
        match ty.id {
            TypeDefId::Struct(id) => {
                let Some(data) = items.struct_data(id) else {
                    return Ok(None);
                };
                Ok(Some((data.owner, data.name.as_str())))
            }
            TypeDefId::Enum(id) => {
                let Some(data) = items.enum_data(id) else {
                    return Ok(None);
                };
                Ok(Some((data.owner, data.name.as_str())))
            }
            TypeDefId::Union(id) => {
                let Some(data) = items.union_data(id) else {
                    return Ok(None);
                };
                Ok(Some((data.owner, data.name.as_str())))
            }
        }
    }
}
