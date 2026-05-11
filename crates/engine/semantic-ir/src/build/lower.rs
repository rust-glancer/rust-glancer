//! Lowers resolved module items into the semantic signature graph.
//!
//! The def-map owns name-resolution identity, while the item tree owns syntax-shaped declarations.
//! This pass joins those two views into stable semantic items that later query layers can use
//! without walking AST or module scopes again.

use anyhow::Context as _;

use rg_def_map::{
    DefMapDb, DefMapReadTxn, LocalDefRef, LocalImplRef, ModuleRef, PackageSlot, TargetRef,
};
use rg_item_tree::{
    ConstItem, FunctionItem, ImplItem, ItemKind, ItemNode, ItemTreeDb, ItemTreeId, ItemTreeRef,
    Package as ItemTreePackage, StaticItem, TraitItem, TypeAliasItem,
};
use rg_parse::{FileId, TargetId};
use rg_text::Name;

use crate::{
    ConstData, EnumData, FunctionData, ImplData, PackageIr, StaticData, StructData, TargetIr,
    TraitData, TypeAliasData, UnionData,
    ids::{
        AssocItemId, ConstId, FunctionId, ImplId, ItemId, ItemOwner, StaticId, TraitId, TypeAliasId,
    },
    signature::{ConstSignature, FunctionSignature, TypeAliasSignature},
};

pub(super) fn build_packages(
    item_tree: &ItemTreeDb,
    def_map: &DefMapDb,
) -> anyhow::Result<Vec<PackageIr>> {
    let mut packages = Vec::with_capacity(def_map.package_count());

    for package_idx in 0..def_map.package_count() {
        packages.push(build_package(item_tree, def_map, PackageSlot(package_idx))?);
    }

    Ok(packages)
}

pub(super) fn build_package(
    item_tree: &ItemTreeDb,
    def_map: &DefMapDb,
    package: PackageSlot,
) -> anyhow::Result<PackageIr> {
    let def_map_package = def_map
        .resident_package(package)
        .with_context(|| format!("while attempting to fetch def-map package {}", package.0))?;
    let item_tree_package = item_tree
        .package(package.0)
        .with_context(|| format!("while attempting to fetch item tree package {}", package.0))?;
    let mut targets = Vec::with_capacity(def_map_package.targets().len());
    let def_map_txn = def_map.read_txn(super::unexpected_package_loader());

    for (target_idx, _) in def_map_package.targets().iter().enumerate() {
        let target_ref = TargetRef {
            package,
            target: TargetId(target_idx),
        };
        targets.push(
            TargetLowering::new(item_tree_package, target_ref, &def_map_txn)?
                .lower()
                .with_context(|| {
                    format!("while attempting to lower semantic IR for target {target_idx}")
                })?,
        );
    }

    Ok(PackageIr::new(targets))
}

struct TargetLowering<'a, 'db> {
    item_tree: &'a ItemTreePackage,
    target: TargetRef,
    def_map: &'a DefMapReadTxn<'db>,
    target_ir: TargetIr,
}

impl<'a, 'db> TargetLowering<'a, 'db> {
    fn new(
        item_tree: &'a ItemTreePackage,
        target: TargetRef,
        def_map: &'a DefMapReadTxn<'db>,
    ) -> anyhow::Result<Self> {
        let local_def_count = def_map
            .local_defs(target)
            .with_context(|| {
                format!(
                    "while attempting to fetch def-map local definitions for target {:?}",
                    target.target,
                )
            })?
            .len();

        Ok(Self {
            item_tree,
            target,
            def_map,
            target_ir: TargetIr::new(local_def_count),
        })
    }

    fn lower(mut self) -> anyhow::Result<TargetIr> {
        // Local definitions already come from the def-map, so lowering follows def-map identity
        // order and only asks the item tree for declaration payloads.
        let local_defs = self
            .def_map
            .local_defs(self.target)
            .with_context(|| {
                format!(
                    "while attempting to fetch def-map local definitions for target {:?}",
                    self.target.target,
                )
            })?
            .into_iter()
            .map(|(local_def_ref, local_def)| (local_def_ref, local_def.source, local_def.module))
            .collect::<Vec<_>>();
        for (local_def_ref, source, module) in local_defs {
            let item = self.item(source)?;
            let owner = ModuleRef {
                target: self.target,
                module,
            };

            if let Some(item_id) = self.lower_local_item(local_def_ref, source, owner, item) {
                self.target_ir
                    .set_local_item(local_def_ref.local_def, item_id);
            }
        }

        // Impl blocks are not namespace bindings, so they travel through a separate def-map table
        // and are lowered after named items have their semantic ids.
        let local_impls = self
            .def_map
            .local_impls(self.target)
            .with_context(|| {
                format!(
                    "while attempting to fetch def-map local impls for target {:?}",
                    self.target.target,
                )
            })?
            .into_iter()
            .map(|(local_impl_ref, local_impl)| {
                (local_impl_ref, local_impl.source, local_impl.module)
            })
            .collect::<Vec<_>>();
        for (local_impl_ref, source, module) in local_impls {
            let item = self.item(source)?;
            let owner = ModuleRef {
                target: self.target,
                module,
            };

            if let ItemKind::Impl(impl_item) = &item.kind {
                let impl_id = self.lower_impl(local_impl_ref, source, owner, impl_item);
                self.target_ir.push_local_impl(impl_id);
            }
        }

        Ok(self.target_ir)
    }

    fn item(&self, item_ref: ItemTreeRef) -> anyhow::Result<&'a ItemNode> {
        self.item_tree.item(item_ref).with_context(|| {
            format!(
                "while attempting to fetch item-tree node {:?} in {:?}",
                item_ref.item, item_ref.file_id
            )
        })
    }

    fn lower_local_item(
        &mut self,
        local_def: LocalDefRef,
        source: ItemTreeRef,
        owner: ModuleRef,
        item: &ItemNode,
    ) -> Option<ItemId> {
        // Imports, modules, and unsupported syntax already did their def-map work. Semantic IR
        // keeps only declarations that carry signature facts or item identities for queries.
        match &item.kind {
            ItemKind::Const(const_item) => Some(ItemId::Const(self.lower_const(
                Some(local_def),
                source,
                ItemOwner::Module(owner),
                item,
                const_item,
            ))),
            ItemKind::Enum(enum_item) => {
                let id = self.target_ir.items_mut().alloc_enum(EnumData {
                    local_def,
                    source,
                    owner,
                    name: item.name.clone()?,
                    visibility: item.visibility.clone(),
                    docs: item.docs.clone(),
                    generics: enum_item.generics.clone(),
                    variants: enum_item.variants.clone(),
                });
                Some(ItemId::Enum(id))
            }
            ItemKind::Function(function_item) => Some(ItemId::Function(self.lower_function(
                Some(local_def),
                source,
                ItemOwner::Module(owner),
                item,
                function_item,
            ))),
            ItemKind::Static(static_item) => Some(ItemId::Static(self.lower_static(
                local_def,
                source,
                owner,
                item,
                static_item,
            ))),
            ItemKind::Struct(struct_item) => {
                let id = self.target_ir.items_mut().alloc_struct(StructData {
                    local_def,
                    source,
                    owner,
                    name: item.name.clone()?,
                    visibility: item.visibility.clone(),
                    docs: item.docs.clone(),
                    generics: struct_item.generics.clone(),
                    fields: struct_item.fields.clone(),
                });
                Some(ItemId::Struct(id))
            }
            ItemKind::Trait(trait_item) => Some(ItemId::Trait(
                self.lower_trait(local_def, source, owner, item, trait_item),
            )),
            ItemKind::TypeAlias(type_alias) => Some(ItemId::TypeAlias(self.lower_type_alias(
                Some(local_def),
                source,
                ItemOwner::Module(owner),
                item,
                type_alias,
            ))),
            ItemKind::Union(union_item) => {
                let id = self.target_ir.items_mut().alloc_union(UnionData {
                    local_def,
                    source,
                    owner,
                    name: item.name.clone()?,
                    visibility: item.visibility.clone(),
                    docs: item.docs.clone(),
                    generics: union_item.generics.clone(),
                    fields: union_item.fields.clone(),
                });
                Some(ItemId::Union(id))
            }
            _ => None,
        }
    }

    fn lower_trait(
        &mut self,
        local_def: LocalDefRef,
        source: ItemTreeRef,
        owner: ModuleRef,
        item: &ItemNode,
        trait_item: &TraitItem,
    ) -> TraitId {
        // Allocate first so associated items can point back at this trait as their owner.
        let trait_id = self.target_ir.items_mut().alloc_trait(TraitData {
            local_def,
            source,
            owner,
            name: item.name.clone().unwrap_or_else(|| Name::new("<missing>")),
            visibility: item.visibility.clone(),
            docs: item.docs.clone(),
            generics: trait_item.generics.clone(),
            super_traits: trait_item.super_traits.clone(),
            items: Vec::new(),
            is_unsafe: trait_item.is_unsafe,
        });
        let assoc_items = self.lower_assoc_items(
            source.file_id,
            &trait_item.items,
            ItemOwner::Trait(trait_id),
        );
        self.target_ir.items_mut().traits[trait_id].items = assoc_items;
        trait_id
    }

    fn lower_impl(
        &mut self,
        local_impl: LocalImplRef,
        source: ItemTreeRef,
        owner: ModuleRef,
        impl_item: &ImplItem,
    ) -> ImplId {
        // Impl header paths are stored as syntax here; semantic resolution fills the resolved
        // self/trait ids once every target's item store exists.
        let impl_id = self.target_ir.items_mut().alloc_impl(ImplData {
            local_impl,
            source,
            owner,
            generics: impl_item.generics.clone(),
            trait_ref: impl_item.trait_ref.clone(),
            self_ty: impl_item.self_ty.clone(),
            resolved_self_tys: Vec::new(),
            resolved_trait_refs: Vec::new(),
            items: Vec::new(),
            is_unsafe: impl_item.is_unsafe,
        });
        let assoc_items =
            self.lower_assoc_items(source.file_id, &impl_item.items, ItemOwner::Impl(impl_id));
        self.target_ir.items_mut().impls[impl_id].items = assoc_items;
        impl_id
    }

    fn lower_assoc_items(
        &mut self,
        file_id: FileId,
        item_ids: &[ItemTreeId],
        owner: ItemOwner,
    ) -> Vec<AssocItemId> {
        let mut assoc_items = Vec::new();

        // Associated items share the same stores as module items, but they do not have def-map
        // local definitions because they are reached through their trait/impl owner.
        for item_id in item_ids {
            let source = ItemTreeRef {
                file_id,
                item: *item_id,
            };
            let Some(item) = self.item_tree.item(source) else {
                continue;
            };

            match &item.kind {
                ItemKind::Const(const_item) => {
                    assoc_items.push(AssocItemId::Const(
                        self.lower_const(None, source, owner, item, const_item),
                    ));
                }
                ItemKind::Function(function_item) => {
                    assoc_items.push(AssocItemId::Function(self.lower_function(
                        None,
                        source,
                        owner,
                        item,
                        function_item,
                    )));
                }
                ItemKind::TypeAlias(type_alias) => {
                    assoc_items.push(AssocItemId::TypeAlias(
                        self.lower_type_alias(None, source, owner, item, type_alias),
                    ));
                }
                _ => {}
            }
        }

        assoc_items
    }

    fn lower_function(
        &mut self,
        local_def: Option<LocalDefRef>,
        source: ItemTreeRef,
        owner: ItemOwner,
        item: &ItemNode,
        declaration: &FunctionItem,
    ) -> FunctionId {
        self.target_ir.items_mut().alloc_function(FunctionData {
            local_def,
            source,
            span: item.span,
            name_span: item.name_span,
            owner,
            name: item.name.clone().unwrap_or_else(|| Name::new("<missing>")),
            visibility: item.visibility.clone(),
            docs: item.docs.clone(),
            signature: FunctionSignature::from_item(declaration),
        })
    }

    fn lower_type_alias(
        &mut self,
        local_def: Option<LocalDefRef>,
        source: ItemTreeRef,
        owner: ItemOwner,
        item: &ItemNode,
        declaration: &TypeAliasItem,
    ) -> TypeAliasId {
        self.target_ir.items_mut().alloc_type_alias(TypeAliasData {
            local_def,
            source,
            span: item.span,
            name_span: item.name_span,
            owner,
            name: item.name.clone().unwrap_or_else(|| Name::new("<missing>")),
            visibility: item.visibility.clone(),
            docs: item.docs.clone(),
            signature: TypeAliasSignature::from_item(declaration),
        })
    }

    fn lower_const(
        &mut self,
        local_def: Option<LocalDefRef>,
        source: ItemTreeRef,
        owner: ItemOwner,
        item: &ItemNode,
        declaration: &ConstItem,
    ) -> ConstId {
        self.target_ir.items_mut().alloc_const(ConstData {
            local_def,
            source,
            span: item.span,
            name_span: item.name_span,
            owner,
            name: item.name.clone().unwrap_or_else(|| Name::new("<missing>")),
            visibility: item.visibility.clone(),
            docs: item.docs.clone(),
            signature: ConstSignature::from_item(declaration),
        })
    }

    fn lower_static(
        &mut self,
        local_def: LocalDefRef,
        source: ItemTreeRef,
        owner: ModuleRef,
        item: &ItemNode,
        declaration: &StaticItem,
    ) -> StaticId {
        self.target_ir.items_mut().alloc_static(StaticData {
            local_def,
            source,
            span: item.span,
            name_span: item.name_span,
            owner,
            name: item.name.clone().unwrap_or_else(|| Name::new("<missing>")),
            visibility: item.visibility.clone(),
            docs: item.docs.clone(),
            ty: declaration.ty.clone(),
            mutability: declaration.mutability,
        })
    }
}
