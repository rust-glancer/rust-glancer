//! Lowers resolved module items into the semantic signature graph.
//!
//! The def-map owns name-resolution identity, while the item tree owns syntax-shaped declarations.
//! This pass joins those two views into stable semantic items that later query layers can use
//! without walking AST or module scopes again.

use anyhow::Context as _;

use rg_def_map::{DefMapDb, DefMapReadTxn, PackageSlot};
use rg_ir_model::{
    AssocItemId, ConstId, DefMapRef, FunctionId, ItemId, ItemOwner, LocalDefRef, LocalImplRef,
    ModuleRef, StaticId, TargetRef, TraitId, TypeAliasId,
    hir::source::{ItemSource, ItemSourceKind},
    hir::{
        items::{
            ConstData, EnumData, FunctionData, ImplData, StaticData, StructData, TraitData,
            TypeAliasData, UnionData,
        },
        signature::{ConstSignature, FunctionSignature, TypeAliasSignature},
    },
};
use rg_item_tree::{
    ConstItem, FunctionItem, ImplItem, ItemKind, ItemNode, ItemTreeDb, ItemTreeId,
    Package as ItemTreePackage, StaticItem, TraitItem, TypeAliasItem,
};
use rg_parse::TargetId;
use rg_text::Name;

use crate::{ItemStore, PackageIr, item_store::ItemStoreBuilder};

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
    let mut targets = Vec::with_capacity(def_map_package.def_maps().len());
    let def_map_txn = def_map.read_txn(super::unexpected_package_loader());

    for (target_idx, _) in def_map_package.def_maps().iter().enumerate() {
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
    def_map_txn: &'a DefMapReadTxn<'db>,
    items: ItemStoreBuilder,
}

impl<'a, 'db> TargetLowering<'a, 'db> {
    fn new(
        item_tree: &'a ItemTreePackage,
        target: TargetRef,
        def_map_txn: &'a DefMapReadTxn<'db>,
    ) -> anyhow::Result<Self> {
        let def_map = def_map_txn.def_map(target).with_context(|| {
            format!(
                "while attempting to fetch def-map local definitions for target {:?}",
                target.target,
            )
        })?;

        let local_def_count = def_map
            .map(|def_map| def_map.local_defs().len())
            .unwrap_or_default();

        Ok(Self {
            item_tree,
            target,
            def_map_txn,
            items: ItemStoreBuilder::new(DefMapRef::Target(target), local_def_count),
        })
    }

    fn lower(mut self) -> anyhow::Result<ItemStore> {
        // Local definitions already come from the def-map, so lowering follows def-map identity
        // order and only asks the item tree for declaration payloads.
        let def_map = self
            .def_map_txn
            .def_map(self.target)
            .with_context(|| {
                format!(
                    "while attempting to fetch def-map local definitions for target {:?}",
                    self.target.target,
                )
            })?
            .context("No defmap to lower from")?;
        for local_def_ref in def_map.local_def_refs() {
            let local_def = def_map.local_def(local_def_ref.local_def).unwrap();

            let item = self.item(local_def.source)?;
            let owner = ModuleRef {
                origin: DefMapRef::Target(self.target),
                module: local_def.module,
            };

            if let Some(item_id) =
                self.lower_local_item(local_def_ref, local_def.source, owner, item)
            {
                self.items.set_local_item(local_def_ref.local_def, item_id);
            }
        }

        // Impl blocks are not namespace bindings, so they travel through a separate def-map table
        // and are lowered after named items have their semantic ids.
        for local_impl_ref in def_map.lodal_impl_refs() {
            let local_impl = def_map.local_impl(local_impl_ref.local_impl).unwrap();
            let item = self.item(local_impl.source)?;
            let owner = ModuleRef {
                origin: DefMapRef::Target(self.target),
                module: local_impl.module,
            };

            if let ItemKind::Impl(impl_item) = &item.kind {
                self.lower_impl(local_impl_ref, local_impl.source, owner, impl_item);
            }
        }

        Ok(self.items.build())
    }

    fn item(&self, source: ItemSource) -> anyhow::Result<&'a ItemNode> {
        let item = match source.kind {
            ItemSourceKind::ItemTree(item_ref) => {
                self.item_tree.item(item_ref).with_context(|| {
                    format!(
                        "while attempting to fetch item-tree node {:?} in {:?}",
                        item_ref.item, item_ref.file_id
                    )
                })?
            }
            ItemSourceKind::Generated(item_ref) => self
                .def_map_txn
                .def_map(self.target)
                .with_context(|| {
                    format!(
                        "while attempting to fetch generated item {:?} from generated source {:?}",
                        item_ref.item, item_ref.source
                    )
                })?
                .and_then(|def_map| def_map.generated_source(item_ref.source))
                .and_then(|source| source.item(item_ref.item))
                .with_context(|| {
                    format!(
                        "while attempting to find generated item {:?} from generated source {:?}",
                        item_ref.item, item_ref.source
                    )
                })?,
            ItemSourceKind::Body(_) => anyhow::bail!("Body is not supported"),
        };
        Ok(item)
    }

    fn lower_local_item(
        &mut self,
        local_def: LocalDefRef,
        source: ItemSource,
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
                let id = self.items.enums.alloc(EnumData {
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
                let id = self.items.structs.alloc(StructData {
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
                let id = self.items.unions.alloc(UnionData {
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
        source: ItemSource,
        owner: ModuleRef,
        item: &ItemNode,
        trait_item: &TraitItem,
    ) -> TraitId {
        // Allocate first so associated items can point back at this trait as their owner.
        let trait_id = self.items.traits.alloc(TraitData {
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
        let assoc_items =
            self.lower_assoc_items(source, &trait_item.items, ItemOwner::Trait(trait_id));
        self.items.traits[trait_id].items = assoc_items;
        trait_id
    }

    fn lower_impl(
        &mut self,
        local_impl: LocalImplRef,
        source: ItemSource,
        owner: ModuleRef,
        impl_item: &ImplItem,
    ) {
        // Impl header paths are stored as syntax here; semantic resolution fills the resolved
        // self/trait ids once every target's item store exists.
        let impl_id = self.items.impls.alloc(ImplData {
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
            self.lower_assoc_items(source, &impl_item.items, ItemOwner::Impl(impl_id));
        self.items.impls[impl_id].items = assoc_items;
    }

    fn lower_assoc_items(
        &mut self,
        parent_source: ItemSource,
        item_ids: &[ItemTreeId],
        owner: ItemOwner,
    ) -> Vec<AssocItemId> {
        let mut assoc_items = Vec::new();

        // Associated items share the same stores as module items, but they do not have def-map
        // local definitions because they are reached through their trait/impl owner.
        for item_id in item_ids {
            let source = parent_source.with_item(*item_id);
            let Ok(item) = self.item(source) else {
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
        source: ItemSource,
        owner: ItemOwner,
        item: &ItemNode,
        declaration: &FunctionItem,
    ) -> FunctionId {
        self.items.functions.alloc(FunctionData {
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
        source: ItemSource,
        owner: ItemOwner,
        item: &ItemNode,
        declaration: &TypeAliasItem,
    ) -> TypeAliasId {
        self.items.type_aliases.alloc(TypeAliasData {
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
        source: ItemSource,
        owner: ItemOwner,
        item: &ItemNode,
        declaration: &ConstItem,
    ) -> ConstId {
        self.items.consts.alloc(ConstData {
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
        source: ItemSource,
        owner: ModuleRef,
        item: &ItemNode,
        declaration: &StaticItem,
    ) -> StaticId {
        self.items.statics.alloc(StaticData {
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
