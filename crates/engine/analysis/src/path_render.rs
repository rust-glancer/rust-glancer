//! Renders stable analysis identities as user-facing Rust paths.
//!
//! Hovers need a compact "where is this declared?" label. The renderer follows DefMap module
//! parents and Semantic IR owners, but it does not try to reconstruct import aliases or
//! rustdoc-style canonical paths.

use rg_def_map::{ModuleRef, TargetRef};
use rg_semantic_ir::{
    ConstRef, FunctionRef, ImplId, ItemOwner, StaticRef, TraitRef, TypeAliasRef, TypeDefId,
    TypeDefRef,
};

use super::Analysis;

pub(super) struct PathRenderer<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> PathRenderer<'a, 'db> {
    pub(super) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(super) fn module_path(&self, module_ref: ModuleRef) -> anyhow::Result<Option<String>> {
        let package = self.0.def_map.package(module_ref.target.package)?;
        let Some(def_map) = self.0.def_map.def_map(module_ref.target)? else {
            return Ok(None);
        };
        let mut names = Vec::new();
        let mut current = module_ref.module;

        // Module ids form a parent chain rooted at the target module. Walking it upward and then
        // reversing gives us the same crate::module::child shape users see in Rust paths.
        loop {
            let Some(module) = def_map.module(current) else {
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
                .target_name(module_ref.target.target)
                .unwrap_or_else(|| package.package_name())
                .to_string(),
        );
        names.reverse();
        Ok(Some(names.join("::")))
    }

    pub(super) fn type_def_path(&self, ty: TypeDefRef) -> anyhow::Result<Option<String>> {
        let Some((module, name)) = self.type_def_owner_and_name(ty)? else {
            return Ok(None);
        };
        self.path_in_module(module, name)
    }

    pub(super) fn trait_path(&self, trait_ref: TraitRef) -> anyhow::Result<Option<String>> {
        let Some(data) = self.0.semantic_ir.trait_data(trait_ref)? else {
            return Ok(None);
        };
        self.path_in_module(data.owner, &data.name)
    }

    pub(super) fn function_path(
        &self,
        function_ref: FunctionRef,
    ) -> anyhow::Result<Option<String>> {
        let Some(data) = self.0.semantic_ir.function_data(function_ref)? else {
            return Ok(None);
        };
        Ok(self
            .path_for_owner(function_ref.target, data.owner)?
            .map(|owner| format!("{owner}::{}", data.name)))
    }

    pub(super) fn type_alias_path(
        &self,
        type_alias_ref: TypeAliasRef,
    ) -> anyhow::Result<Option<String>> {
        let Some(data) = self.0.semantic_ir.type_alias_data(type_alias_ref)? else {
            return Ok(None);
        };
        Ok(self
            .path_for_owner(type_alias_ref.target, data.owner)?
            .map(|owner| format!("{owner}::{}", data.name)))
    }

    pub(super) fn const_path(&self, const_ref: ConstRef) -> anyhow::Result<Option<String>> {
        let Some(data) = self.0.semantic_ir.const_data(const_ref)? else {
            return Ok(None);
        };
        Ok(self
            .path_for_owner(const_ref.target, data.owner)?
            .map(|owner| format!("{owner}::{}", data.name)))
    }

    pub(super) fn static_path(&self, static_ref: StaticRef) -> anyhow::Result<Option<String>> {
        let Some(data) = self.0.semantic_ir.static_data(static_ref)? else {
            return Ok(None);
        };
        self.path_in_module(data.owner, &data.name)
    }

    pub(super) fn enum_variant_path(
        &self,
        variant_ref: rg_semantic_ir::EnumVariantRef,
    ) -> anyhow::Result<Option<String>> {
        let Some(data) = self.0.semantic_ir.enum_variant_data(variant_ref)? else {
            return Ok(None);
        };
        Ok(self
            .type_def_path(data.owner)?
            .map(|owner| format!("{owner}::{}", data.variant.name)))
    }

    fn path_in_module(&self, module_ref: ModuleRef, name: &str) -> anyhow::Result<Option<String>> {
        Ok(self
            .module_path(module_ref)?
            .map(|module| format!("{module}::{name}")))
    }

    fn path_for_owner(
        &self,
        target: TargetRef,
        owner: ItemOwner,
    ) -> anyhow::Result<Option<String>> {
        match owner {
            ItemOwner::Module(module_ref) => self.module_path(module_ref),
            ItemOwner::Trait(trait_id) => self.trait_path(TraitRef {
                target,
                id: trait_id,
            }),
            ItemOwner::Impl(impl_id) => self.impl_self_path(target, impl_id),
        }
    }

    fn impl_self_path(&self, target: TargetRef, impl_id: ImplId) -> anyhow::Result<Option<String>> {
        let Some(data) = self.0.semantic_ir.impl_data(rg_semantic_ir::ImplRef {
            target,
            id: impl_id,
        })?
        else {
            return Ok(None);
        };

        if let Some(ty) = data.resolved_self_tys.first()
            && let Some(path) = self.type_def_path(*ty)?
        {
            return Ok(Some(path));
        }

        self.module_path(data.owner)
    }

    fn type_def_owner_and_name(&self, ty: TypeDefRef) -> anyhow::Result<Option<(ModuleRef, &str)>> {
        let Some(target_ir) = self.0.semantic_ir.target_ir(ty.target)? else {
            return Ok(None);
        };
        match ty.id {
            TypeDefId::Struct(id) => {
                let Some(data) = target_ir.items().struct_data(id) else {
                    return Ok(None);
                };
                Ok(Some((data.owner, data.name.as_str())))
            }
            TypeDefId::Enum(id) => {
                let Some(data) = target_ir.items().enum_data(id) else {
                    return Ok(None);
                };
                Ok(Some((data.owner, data.name.as_str())))
            }
            TypeDefId::Union(id) => {
                let Some(data) = target_ir.items().union_data(id) else {
                    return Ok(None);
                };
                Ok(Some((data.owner, data.name.as_str())))
            }
        }
    }
}
