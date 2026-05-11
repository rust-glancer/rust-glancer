//! Read transactions over frozen Semantic IR package data.

use rg_def_map::{DefMapReadTxn, LocalDefRef, ModuleRef, PackageSlot, Path, TargetRef};
use rg_item_tree::FieldKey;
use rg_package_store::{PackageRead, PackageStoreError, PackageStoreReadTxn};
use rg_parse::TargetId;

use crate::{
    AssocItemId, ConstData, ConstRef, EnumData, EnumVariantData, EnumVariantRef, FieldData,
    FieldRef, FunctionData, FunctionRef, ImplData, ImplRef, ItemId, ItemOwner, PackageIr,
    SemanticTypePathResolution, StaticData, StaticRef, StructData, TargetIr, TraitData,
    TraitImplRef, TraitRef, TypeAliasData, TypeAliasRef, TypeDefId, TypeDefRef, TypePathContext,
    UnionData, push_unique,
};

/// Read-only semantic IR access for one query transaction.
#[derive(Debug, Clone)]
pub struct SemanticIrReadTxn<'db> {
    packages: PackageStoreReadTxn<'db, PackageIr>,
}

impl<'db> SemanticIrReadTxn<'db> {
    pub(crate) fn from_package_store(packages: PackageStoreReadTxn<'db, PackageIr>) -> Self {
        Self { packages }
    }

    pub fn package(
        &self,
        package: PackageSlot,
    ) -> Result<PackageRead<'_, PackageIr>, PackageStoreError> {
        self.packages.read(package)
    }

    pub fn target_ir(&self, target: TargetRef) -> Result<Option<&TargetIr>, PackageStoreError> {
        let package = self.package(target.package)?;
        Ok(package.into_ref().target(target.target))
    }

    pub fn materialize_included_target_irs(
        &self,
    ) -> Result<Vec<(TargetRef, &TargetIr)>, PackageStoreError> {
        let target_irs = self
            .packages
            .materialize_included_packages_with_slots()?
            .into_iter()
            .flat_map(|(package_slot, package)| {
                let package = package.into_ref();
                package
                    .targets()
                    .iter()
                    .enumerate()
                    .map(move |(target_idx, target_ir)| {
                        (
                            TargetRef {
                                package: package_slot,
                                target: TargetId(target_idx),
                            },
                            target_ir,
                        )
                    })
            })
            .collect::<Vec<_>>();

        Ok(target_irs)
    }

    pub fn structs(
        &self,
        target: TargetRef,
    ) -> Result<Vec<(TypeDefRef, &StructData)>, PackageStoreError> {
        let structs = self
            .target_ir(target)?
            .into_iter()
            .flat_map(move |target_ir| {
                target_ir
                    .items()
                    .structs
                    .iter_with_ids()
                    .map(move |(id, data)| {
                        (
                            TypeDefRef {
                                target,
                                id: TypeDefId::Struct(id),
                            },
                            data,
                        )
                    })
            })
            .collect::<Vec<_>>();

        Ok(structs)
    }

    pub fn unions(
        &self,
        target: TargetRef,
    ) -> Result<Vec<(TypeDefRef, &UnionData)>, PackageStoreError> {
        let unions = self
            .target_ir(target)?
            .into_iter()
            .flat_map(move |target_ir| {
                target_ir
                    .items()
                    .unions
                    .iter_with_ids()
                    .map(move |(id, data)| {
                        (
                            TypeDefRef {
                                target,
                                id: TypeDefId::Union(id),
                            },
                            data,
                        )
                    })
            })
            .collect::<Vec<_>>();

        Ok(unions)
    }

    pub fn enums(
        &self,
        target: TargetRef,
    ) -> Result<Vec<(TypeDefRef, &EnumData)>, PackageStoreError> {
        let enums = self
            .target_ir(target)?
            .into_iter()
            .flat_map(move |target_ir| {
                target_ir
                    .items()
                    .enums
                    .iter_with_ids()
                    .map(move |(id, data)| {
                        (
                            TypeDefRef {
                                target,
                                id: TypeDefId::Enum(id),
                            },
                            data,
                        )
                    })
            })
            .collect::<Vec<_>>();

        Ok(enums)
    }

    pub fn traits(
        &self,
        target: TargetRef,
    ) -> Result<Vec<(TraitRef, &TraitData)>, PackageStoreError> {
        let traits = self
            .target_ir(target)?
            .into_iter()
            .flat_map(move |target_ir| {
                target_ir
                    .items()
                    .traits
                    .iter_with_ids()
                    .map(move |(id, data)| (TraitRef { target, id }, data))
            })
            .collect::<Vec<_>>();

        Ok(traits)
    }

    pub fn impls(&self, target: TargetRef) -> Result<Vec<(ImplRef, &ImplData)>, PackageStoreError> {
        let impls = self
            .target_ir(target)?
            .into_iter()
            .flat_map(move |target_ir| {
                target_ir
                    .items()
                    .impls
                    .iter_with_ids()
                    .map(move |(id, data)| (ImplRef { target, id }, data))
            })
            .collect::<Vec<_>>();

        Ok(impls)
    }

    pub fn functions(
        &self,
        target: TargetRef,
    ) -> Result<Vec<(FunctionRef, &FunctionData)>, PackageStoreError> {
        let functions = self
            .target_ir(target)?
            .into_iter()
            .flat_map(move |target_ir| {
                target_ir
                    .items()
                    .functions
                    .iter_with_ids()
                    .map(move |(id, data)| (FunctionRef { target, id }, data))
            })
            .collect::<Vec<_>>();

        Ok(functions)
    }

    pub fn type_aliases(
        &self,
        target: TargetRef,
    ) -> Result<Vec<(TypeAliasRef, &TypeAliasData)>, PackageStoreError> {
        let aliases = self
            .target_ir(target)?
            .into_iter()
            .flat_map(move |target_ir| {
                target_ir
                    .items()
                    .type_aliases
                    .iter_with_ids()
                    .map(move |(id, data)| (TypeAliasRef { target, id }, data))
            })
            .collect::<Vec<_>>();

        Ok(aliases)
    }

    pub fn consts(
        &self,
        target: TargetRef,
    ) -> Result<Vec<(ConstRef, &ConstData)>, PackageStoreError> {
        let consts = self
            .target_ir(target)?
            .into_iter()
            .flat_map(move |target_ir| {
                target_ir
                    .items()
                    .consts
                    .iter_with_ids()
                    .map(move |(id, data)| (ConstRef { target, id }, data))
            })
            .collect::<Vec<_>>();

        Ok(consts)
    }

    pub fn statics(
        &self,
        target: TargetRef,
    ) -> Result<Vec<(StaticRef, &StaticData)>, PackageStoreError> {
        let statics = self
            .target_ir(target)?
            .into_iter()
            .flat_map(move |target_ir| {
                target_ir
                    .items()
                    .statics
                    .iter_with_ids()
                    .map(move |(id, data)| (StaticRef { target, id }, data))
            })
            .collect::<Vec<_>>();

        Ok(statics)
    }

    pub fn resolve_type_path(
        &self,
        def_map: &DefMapReadTxn<'db>,
        context: TypePathContext,
        path: &Path,
    ) -> Result<SemanticTypePathResolution, PackageStoreError> {
        if path.is_self_type() {
            let Some(impl_ref) = context.impl_ref else {
                return Ok(SemanticTypePathResolution::Unknown);
            };
            let types = self
                .impl_data(impl_ref)?
                .map(|data| data.resolved_self_tys.clone())
                .unwrap_or_default();
            return Ok(if types.is_empty() {
                SemanticTypePathResolution::Unknown
            } else {
                SemanticTypePathResolution::SelfType(types)
            });
        }

        let type_defs = self.type_defs_for_path(def_map, context.module, path)?;
        if type_defs.is_empty() {
            let traits = self.traits_for_path(def_map, context.module, path)?;
            Ok(if traits.is_empty() {
                SemanticTypePathResolution::Unknown
            } else {
                SemanticTypePathResolution::Traits(traits)
            })
        } else {
            Ok(SemanticTypePathResolution::TypeDefs(type_defs))
        }
    }

    pub fn type_defs_for_path(
        &self,
        def_map: &DefMapReadTxn<'db>,
        from: ModuleRef,
        path: &Path,
    ) -> Result<Vec<TypeDefRef>, PackageStoreError> {
        self.resolve_path(def_map, from, path, |db, def| {
            let rg_def_map::DefId::Local(local_def) = def else {
                return Ok(None);
            };

            db.type_def_for_local_def(local_def)
        })
    }

    pub fn traits_for_path(
        &self,
        def_map: &DefMapReadTxn<'db>,
        from: ModuleRef,
        path: &Path,
    ) -> Result<Vec<TraitRef>, PackageStoreError> {
        self.resolve_path(def_map, from, path, |db, def| {
            let rg_def_map::DefId::Local(local_def) = def else {
                return Ok(None);
            };

            db.trait_for_local_def(local_def)
        })
    }

    fn resolve_path<T: PartialEq>(
        &self,
        def_map: &DefMapReadTxn<'db>,
        owner: ModuleRef,
        path: &Path,
        map_def: impl Fn(&Self, rg_def_map::DefId) -> Result<Option<T>, PackageStoreError>,
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

    pub fn type_path_context_for_function(
        &self,
        function_ref: FunctionRef,
    ) -> Result<Option<TypePathContext>, PackageStoreError> {
        let Some(function_data) = self.function_data(function_ref)? else {
            return Ok(None);
        };
        self.type_path_context_for_owner(function_ref.target, function_data.owner)
    }

    pub fn type_path_context_for_owner(
        &self,
        target: TargetRef,
        owner: ItemOwner,
    ) -> Result<Option<TypePathContext>, PackageStoreError> {
        match owner {
            ItemOwner::Module(module_ref) => Ok(Some(TypePathContext::module(module_ref))),
            ItemOwner::Trait(id) => Ok(self
                .trait_data(TraitRef { target, id })?
                .map(|data| TypePathContext::module(data.owner))),
            ItemOwner::Impl(id) => {
                let impl_ref = ImplRef { target, id };
                Ok(self.impl_data(impl_ref)?.map(|data| TypePathContext {
                    module: data.owner,
                    impl_ref: Some(impl_ref),
                }))
            }
        }
    }

    pub fn type_def_for_local_def(
        &self,
        def: LocalDefRef,
    ) -> Result<Option<TypeDefRef>, PackageStoreError> {
        let Some(target_ir) = self.target_ir(def.target)? else {
            return Ok(None);
        };
        let Some(item) = target_ir.item_for_local_def(def.local_def) else {
            return Ok(None);
        };
        let id = match item {
            ItemId::Struct(id) => TypeDefId::Struct(id),
            ItemId::Enum(id) => TypeDefId::Enum(id),
            ItemId::Union(id) => TypeDefId::Union(id),
            ItemId::Trait(_)
            | ItemId::Function(_)
            | ItemId::TypeAlias(_)
            | ItemId::Const(_)
            | ItemId::Static(_) => return Ok(None),
        };

        Ok(Some(TypeDefRef {
            target: def.target,
            id,
        }))
    }

    pub fn trait_for_local_def(
        &self,
        def: LocalDefRef,
    ) -> Result<Option<TraitRef>, PackageStoreError> {
        let Some(target_ir) = self.target_ir(def.target)? else {
            return Ok(None);
        };
        let Some(item) = target_ir.item_for_local_def(def.local_def) else {
            return Ok(None);
        };
        let ItemId::Trait(id) = item else {
            return Ok(None);
        };

        Ok(Some(TraitRef {
            target: def.target,
            id,
        }))
    }

    pub fn function_for_local_def(
        &self,
        def: LocalDefRef,
    ) -> Result<Option<FunctionRef>, PackageStoreError> {
        let Some(target_ir) = self.target_ir(def.target)? else {
            return Ok(None);
        };
        let Some(item) = target_ir.item_for_local_def(def.local_def) else {
            return Ok(None);
        };
        let ItemId::Function(id) = item else {
            return Ok(None);
        };

        Ok(Some(FunctionRef {
            target: def.target,
            id,
        }))
    }

    pub fn local_def_for_type_def(
        &self,
        ty: TypeDefRef,
    ) -> Result<Option<LocalDefRef>, PackageStoreError> {
        let Some(target_ir) = self.target_ir(ty.target)? else {
            return Ok(None);
        };
        let local_def = match ty.id {
            TypeDefId::Struct(id) => target_ir.items().struct_data(id).map(|data| data.local_def),
            TypeDefId::Enum(id) => target_ir.items().enum_data(id).map(|data| data.local_def),
            TypeDefId::Union(id) => target_ir.items().union_data(id).map(|data| data.local_def),
        };
        Ok(local_def)
    }

    pub fn generic_params_for_type_def(
        &self,
        ty: TypeDefRef,
    ) -> Result<Option<&rg_item_tree::GenericParams>, PackageStoreError> {
        let Some(target_ir) = self.target_ir(ty.target)? else {
            return Ok(None);
        };
        let generics = match ty.id {
            TypeDefId::Struct(id) => target_ir.items().struct_data(id).map(|data| &data.generics),
            TypeDefId::Enum(id) => target_ir.items().enum_data(id).map(|data| &data.generics),
            TypeDefId::Union(id) => target_ir.items().union_data(id).map(|data| &data.generics),
        };
        Ok(generics)
    }

    pub fn type_def_name(&self, ty: TypeDefRef) -> Result<Option<&str>, PackageStoreError> {
        let Some(target_ir) = self.target_ir(ty.target)? else {
            return Ok(None);
        };
        let name = match ty.id {
            TypeDefId::Struct(id) => target_ir
                .items()
                .struct_data(id)
                .map(|data| data.name.as_str()),
            TypeDefId::Enum(id) => target_ir
                .items()
                .enum_data(id)
                .map(|data| data.name.as_str()),
            TypeDefId::Union(id) => target_ir
                .items()
                .union_data(id)
                .map(|data| data.name.as_str()),
        };
        Ok(name)
    }

    pub fn enum_data_for_type_def(
        &self,
        ty: TypeDefRef,
    ) -> Result<Option<&EnumData>, PackageStoreError> {
        let TypeDefId::Enum(id) = ty.id else {
            return Ok(None);
        };
        Ok(self
            .target_ir(ty.target)?
            .and_then(|target_ir| target_ir.items().enum_data(id)))
    }

    pub fn enum_variant_for_type_def(
        &self,
        ty: TypeDefRef,
        variant_name: &str,
    ) -> Result<Option<(usize, &rg_item_tree::EnumVariantItem)>, PackageStoreError> {
        let Some(data) = self.enum_data_for_type_def(ty)? else {
            return Ok(None);
        };
        Ok(data
            .variants
            .iter()
            .enumerate()
            .find(|(_, variant)| variant.name == variant_name))
    }

    pub fn enum_variant_ref_for_type_def(
        &self,
        ty: TypeDefRef,
        variant_name: &str,
    ) -> Result<Option<EnumVariantRef>, PackageStoreError> {
        let TypeDefId::Enum(enum_id) = ty.id else {
            return Ok(None);
        };
        let Some((index, _)) = self.enum_variant_for_type_def(ty, variant_name)? else {
            return Ok(None);
        };
        Ok(Some(EnumVariantRef {
            target: ty.target,
            enum_id,
            index,
        }))
    }

    pub fn enum_variant_data(
        &self,
        variant_ref: EnumVariantRef,
    ) -> Result<Option<EnumVariantData<'_>>, PackageStoreError> {
        let Some(target_ir) = self.target_ir(variant_ref.target)? else {
            return Ok(None);
        };
        let Some(data) = target_ir.items().enum_data(variant_ref.enum_id) else {
            return Ok(None);
        };
        let Some(variant) = data.variants.get(variant_ref.index) else {
            return Ok(None);
        };

        Ok(Some(EnumVariantData {
            owner: TypeDefRef {
                target: variant_ref.target,
                id: TypeDefId::Enum(variant_ref.enum_id),
            },
            owner_module: data.owner,
            file_id: data.source.file_id,
            variant,
        }))
    }

    pub fn impl_data(&self, impl_ref: ImplRef) -> Result<Option<&ImplData>, PackageStoreError> {
        Ok(self
            .target_ir(impl_ref.target)?
            .and_then(|target_ir| target_ir.items().impl_data(impl_ref.id)))
    }

    pub fn trait_data(&self, trait_ref: TraitRef) -> Result<Option<&TraitData>, PackageStoreError> {
        Ok(self
            .target_ir(trait_ref.target)?
            .and_then(|target_ir| target_ir.items().trait_data(trait_ref.id)))
    }

    pub fn function_data(
        &self,
        function_ref: FunctionRef,
    ) -> Result<Option<&FunctionData>, PackageStoreError> {
        Ok(self
            .target_ir(function_ref.target)?
            .and_then(|target_ir| target_ir.items().function_data(function_ref.id)))
    }

    pub fn type_alias_data(
        &self,
        type_alias_ref: TypeAliasRef,
    ) -> Result<Option<&TypeAliasData>, PackageStoreError> {
        Ok(self
            .target_ir(type_alias_ref.target)?
            .and_then(|target_ir| target_ir.items().type_alias_data(type_alias_ref.id)))
    }

    pub fn const_data(&self, const_ref: ConstRef) -> Result<Option<&ConstData>, PackageStoreError> {
        Ok(self
            .target_ir(const_ref.target)?
            .and_then(|target_ir| target_ir.items().const_data(const_ref.id)))
    }

    pub fn static_data(
        &self,
        static_ref: StaticRef,
    ) -> Result<Option<&StaticData>, PackageStoreError> {
        Ok(self
            .target_ir(static_ref.target)?
            .and_then(|target_ir| target_ir.items().static_data(static_ref.id)))
    }

    pub fn fields_for_type(&self, ty: TypeDefRef) -> Result<Vec<FieldRef>, PackageStoreError> {
        let Some(field_count) = self.field_count_for_type(ty)? else {
            return Ok(Vec::new());
        };

        let fields = (0..field_count)
            .map(|index| FieldRef { owner: ty, index })
            .collect();
        Ok(fields)
    }

    pub fn field_for_type(
        &self,
        ty: TypeDefRef,
        key: &FieldKey,
    ) -> Result<Option<FieldRef>, PackageStoreError> {
        match key {
            FieldKey::Named(_) => {
                for field_ref in self.fields_for_type(ty)? {
                    if self
                        .field_data(field_ref)?
                        .is_some_and(|data| data.field.key.as_ref() == Some(key))
                    {
                        return Ok(Some(field_ref));
                    }
                }
                Ok(None)
            }
            FieldKey::Tuple(index) => {
                let field_ref = FieldRef {
                    owner: ty,
                    index: *index,
                };
                Ok(self
                    .field_data(field_ref)?
                    .is_some_and(|data| data.field.key.as_ref() == Some(key))
                    .then_some(field_ref))
            }
        }
    }

    pub fn field_data(
        &self,
        field_ref: FieldRef,
    ) -> Result<Option<FieldData<'_>>, PackageStoreError> {
        let Some(target_ir) = self.target_ir(field_ref.owner.target)? else {
            return Ok(None);
        };
        let field = match field_ref.owner.id {
            TypeDefId::Struct(id) => {
                let Some(data) = target_ir.items().struct_data(id) else {
                    return Ok(None);
                };
                let Some(field) = data.fields.fields().get(field_ref.index) else {
                    return Ok(None);
                };
                FieldData {
                    owner_module: data.owner,
                    file_id: data.source.file_id,
                    field,
                }
            }
            TypeDefId::Union(id) => {
                let Some(data) = target_ir.items().union_data(id) else {
                    return Ok(None);
                };
                let Some(field) = data.fields.get(field_ref.index) else {
                    return Ok(None);
                };
                FieldData {
                    owner_module: data.owner,
                    file_id: data.source.file_id,
                    field,
                }
            }
            TypeDefId::Enum(_) => return Ok(None),
        };

        Ok(Some(field))
    }

    pub fn impls_for_type(&self, ty: TypeDefRef) -> Result<Vec<ImplRef>, PackageStoreError> {
        let mut impls = Vec::new();

        for impl_ref in self.impl_refs()? {
            let Some(data) = self.impl_data(impl_ref)? else {
                continue;
            };
            if data.resolved_self_tys.contains(&ty) {
                impls.push(impl_ref);
            }
        }

        Ok(impls)
    }

    pub fn inherent_impls_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<Vec<ImplRef>, PackageStoreError> {
        let mut impls = Vec::new();

        for impl_ref in self.impls_for_type(ty)? {
            let Some(data) = self.impl_data(impl_ref)? else {
                continue;
            };
            if data.trait_ref.is_none() {
                impls.push(impl_ref);
            }
        }

        Ok(impls)
    }

    pub fn trait_impls_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<Vec<TraitImplRef>, PackageStoreError> {
        let mut trait_impls = Vec::new();

        for impl_ref in self.impls_for_type(ty)? {
            let Some(data) = self.impl_data(impl_ref)? else {
                continue;
            };

            for trait_ref in &data.resolved_trait_refs {
                push_unique(
                    &mut trait_impls,
                    TraitImplRef {
                        impl_ref,
                        trait_ref: *trait_ref,
                    },
                );
            }
        }

        Ok(trait_impls)
    }

    pub fn traits_for_type(&self, ty: TypeDefRef) -> Result<Vec<TraitRef>, PackageStoreError> {
        let mut traits = Vec::new();

        for trait_impl in self.trait_impls_for_type(ty)? {
            push_unique(&mut traits, trait_impl.trait_ref);
        }

        Ok(traits)
    }

    pub fn trait_functions(
        &self,
        trait_ref: TraitRef,
    ) -> Result<Vec<FunctionRef>, PackageStoreError> {
        let mut functions = Vec::new();
        let Some(data) = self.trait_data(trait_ref)? else {
            return Ok(functions);
        };

        for item in &data.items {
            if let AssocItemId::Function(id) = item {
                push_unique(
                    &mut functions,
                    FunctionRef {
                        target: trait_ref.target,
                        id: *id,
                    },
                );
            }
        }

        Ok(functions)
    }

    pub fn inherent_functions_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<Vec<FunctionRef>, PackageStoreError> {
        let mut functions = Vec::new();

        for impl_ref in self.inherent_impls_for_type(ty)? {
            let Some(data) = self.impl_data(impl_ref)? else {
                continue;
            };

            for item in &data.items {
                if let AssocItemId::Function(id) = item {
                    push_unique(
                        &mut functions,
                        FunctionRef {
                            target: impl_ref.target,
                            id: *id,
                        },
                    );
                }
            }
        }

        Ok(functions)
    }

    pub fn trait_functions_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<Vec<FunctionRef>, PackageStoreError> {
        let mut functions = Vec::new();

        for trait_ref in self.traits_for_type(ty)? {
            for function in self.trait_functions(trait_ref)? {
                push_unique(&mut functions, function);
            }
        }

        Ok(functions)
    }

    pub fn trait_impl_functions_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<Vec<FunctionRef>, PackageStoreError> {
        let mut functions = Vec::new();

        for trait_impl in self.trait_impls_for_type(ty)? {
            let Some(data) = self.impl_data(trait_impl.impl_ref)? else {
                continue;
            };

            for item in &data.items {
                if let AssocItemId::Function(id) = item {
                    push_unique(
                        &mut functions,
                        FunctionRef {
                            target: trait_impl.impl_ref.target,
                            id: *id,
                        },
                    );
                }
            }
        }

        Ok(functions)
    }

    fn field_count_for_type(&self, ty: TypeDefRef) -> Result<Option<usize>, PackageStoreError> {
        let Some(target_ir) = self.target_ir(ty.target)? else {
            return Ok(None);
        };
        let field_count = match ty.id {
            TypeDefId::Struct(id) => target_ir
                .items()
                .struct_data(id)
                .map(|data| data.fields.fields().len()),
            TypeDefId::Union(id) => target_ir
                .items()
                .union_data(id)
                .map(|data| data.fields.len()),
            TypeDefId::Enum(_) => None,
        };
        Ok(field_count)
    }

    fn impl_refs(&self) -> Result<Vec<ImplRef>, PackageStoreError> {
        let mut impl_refs = Vec::new();

        for (target, _) in self.materialize_included_target_irs()? {
            for (impl_ref, _) in self.impls(target)? {
                impl_refs.push(impl_ref);
            }
        }

        Ok(impl_refs)
    }
}
