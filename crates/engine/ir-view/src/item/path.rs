//! Canonical path projection for indexed declarations.
//!
//! This view follows DefMap module parents and Semantic IR owners to produce the stable Rust-ish
//! paths used by hover, completion details, and symbol containers. It intentionally does not try to
//! reconstruct import aliases or rustdoc-style canonicalization.

use std::fmt::Write as _;

use anyhow::Context as _;
use rg_def_map::DefMapSource;
use rg_ir_model::{
    ConstRef, DefMapRef, FunctionRef, ImplId, ImplRef, ItemOwner, ModuleRef, StaticRef,
    TraitDefRef, TypeAliasRef, TypeDefId, TypeDefRef,
};
use rg_semantic_ir::{EnumVariantData, ItemStoreQuery};
use rg_text::RustEdition;

use crate::{IndexedViewDb, display::syntax::SyntaxRenderer};

/// Renders stable Rust-like paths for indexed declarations.
pub struct PathView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
    syntax: SyntaxRenderer,
}

impl<'a, 'db> PathView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>, edition: RustEdition) -> Self {
        Self {
            db,
            syntax: SyntaxRenderer::new(edition),
        }
    }

    /// Returns the full path for a crate-owned module without loading sibling Cargo targets.
    ///
    /// Module parents and the crate name come from the exact crate DefMap. The package name is used
    /// only when the crate slot has no dedicated crate metadata.
    pub fn module_path(&self, module_ref: ModuleRef) -> anyhow::Result<Option<String>> {
        let Some(crate_ref) = module_ref.origin.as_crate_ref() else {
            return Ok(None);
        };
        let crate_data = self
            .db
            .def_map
            .crate_data(crate_ref)
            .context("load crate data for module path")?;
        let mut names = Vec::new();
        let mut current = module_ref.module;

        // Module ids form a parent chain rooted at the crate module. Walking it upward and then
        // reversing gives us the same crate::item::module::child shape users see in Rust paths.
        loop {
            let Some(module) = self.db.module_data(ModuleRef {
                origin: module_ref.origin,
                module: current,
            })?
            else {
                return Ok(None);
            };
            if let Some(name) = &module.name {
                names.push(name.clone());
            }

            let Some(parent) = module.parent else {
                break;
            };
            current = parent;
        }

        let root_name = match crate_data {
            Some(crate_data) => crate_data.name(),
            None => self
                .db
                .def_map
                .package_name(crate_ref.package)
                .context("load package name for module path")?,
        };
        let mut path = self.syntax.identifier(root_name).to_string();
        for name in names.iter().rev() {
            write!(path, "::{}", self.syntax.identifier(name))
                .expect("string writes should not fail");
        }
        Ok(Some(path))
    }

    /// Return the full path for a type definition.
    pub fn type_def_path(&self, ty: TypeDefRef) -> anyhow::Result<Option<String>> {
        let Some((module, name)) = self.type_def_owner_and_name(ty)? else {
            return Ok(None);
        };
        self.path_in_module(module, name)
    }

    /// Return the full path for a trait.
    pub fn trait_path(&self, trait_ref: TraitDefRef) -> anyhow::Result<Option<String>> {
        let Some(data) = ItemStoreQuery::new(self.db).trait_data(trait_ref)? else {
            return Ok(None);
        };
        self.path_in_module(data.owner, &data.name)
    }

    /// Return the full path for a function or method.
    pub fn function_path(&self, function_ref: FunctionRef) -> anyhow::Result<Option<String>> {
        let Some(data) = ItemStoreQuery::new(self.db).function_data(function_ref)? else {
            return Ok(None);
        };
        let name = self.syntax.identifier(&data.name);
        Ok(self
            .path_for_owner(function_ref.origin, data.owner)?
            .map(|owner| format!("{owner}::{name}")))
    }

    /// Return the full path for a type alias.
    pub fn type_alias_path(&self, type_alias_ref: TypeAliasRef) -> anyhow::Result<Option<String>> {
        let Some(data) = ItemStoreQuery::new(self.db).type_alias_data(type_alias_ref)? else {
            return Ok(None);
        };
        let name = self.syntax.identifier(&data.name);
        Ok(self
            .path_for_owner(type_alias_ref.origin, data.owner)?
            .map(|owner| format!("{owner}::{name}")))
    }

    /// Return the full path for a const item.
    pub fn const_path(&self, const_ref: ConstRef) -> anyhow::Result<Option<String>> {
        let Some(data) = ItemStoreQuery::new(self.db).const_data(const_ref)? else {
            return Ok(None);
        };
        let name = self.syntax.identifier(&data.name);
        Ok(self
            .path_for_owner(const_ref.origin, data.owner)?
            .map(|owner| format!("{owner}::{name}")))
    }

    /// Return the full path for a static item.
    pub fn static_path(&self, static_ref: StaticRef) -> anyhow::Result<Option<String>> {
        let Some(data) = ItemStoreQuery::new(self.db).static_data(static_ref)? else {
            return Ok(None);
        };
        self.path_in_module(data.owner, &data.name)
    }

    /// Return the full path for an enum variant.
    pub fn enum_variant_path(&self, data: EnumVariantData<'_>) -> anyhow::Result<Option<String>> {
        let name = self.syntax.identifier(&data.variant.name);
        Ok(self
            .type_def_path(data.owner)?
            .map(|owner| format!("{owner}::{name}")))
    }

    /// Append a local name to a module path.
    pub fn path_in_module(
        &self,
        module_ref: ModuleRef,
        name: &str,
    ) -> anyhow::Result<Option<String>> {
        let name = self.syntax.identifier(name);
        Ok(self
            .module_path(module_ref)?
            .map(|module| format!("{module}::{name}")))
    }

    /// Return the path of the item owner used as a path prefix.
    fn path_for_owner(
        &self,
        origin: DefMapRef,
        owner: ItemOwner,
    ) -> anyhow::Result<Option<String>> {
        match owner {
            ItemOwner::Module(module_ref) => self.module_path(module_ref),
            ItemOwner::Trait(trait_id) => self.trait_path(TraitDefRef {
                origin,
                id: trait_id,
            }),
            ItemOwner::Impl(impl_id) => self.impl_self_path(origin, impl_id),
        }
    }

    /// Return the best display path for an impl owner.
    fn impl_self_path(&self, origin: DefMapRef, impl_id: ImplId) -> anyhow::Result<Option<String>> {
        let Some(data) = ItemStoreQuery::new(self.db).impl_data(ImplRef {
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

    /// Return the owner module and declared name for a type definition.
    fn type_def_owner_and_name(&self, ty: TypeDefRef) -> anyhow::Result<Option<(ModuleRef, &str)>> {
        let item_query = ItemStoreQuery::new(self.db);
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
