//! Read transactions over frozen Semantic IR package data.

use rg_def_map::{DefMapReadTxn, PackageSlot, Path};
use rg_ir_model::hir::items::{
    ConstData, EnumData, EnumVariantData, FieldData, FunctionData, ImplData, StaticData, TraitData,
    TypeAliasData,
};
use rg_ir_model::{
    ConstRef, DefMapRef, EnumVariantRef, FieldRef, FunctionRef, ImplRef, ItemOwner,
    SemanticItemRef, StaticRef, TraitImplRef, TraitRef, TypeAliasRef, TypeDefId, TypeDefRef,
};
use rg_ir_model::{DefId, LocalDefRef, ModuleRef, TargetRef};
use rg_item_tree::FieldKey;
use rg_package_store::{PackageStoreError, PackageStoreReadTxn};

use rg_ir_model::TypePathResolution;

use crate::{ItemStore, PackageIr, TypePathContext, push_unique};

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
            let types = self
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
            db.semantic_item_for_local_def(local_def)
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
                && let Some(data) = self.impl_data(impl_ref)?
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

    pub fn type_path_context_for_function(
        &self,
        function_ref: FunctionRef,
    ) -> Result<Option<TypePathContext>, PackageStoreError> {
        let Some(function_data) = self.function_data(function_ref)? else {
            return Ok(None);
        };
        self.type_path_context_for_owner(function_ref.origin, function_data.owner)
    }

    pub fn type_path_context_for_owner(
        &self,
        origin: DefMapRef,
        owner: ItemOwner,
    ) -> Result<Option<TypePathContext>, PackageStoreError> {
        match owner {
            ItemOwner::Module(module_ref) => Ok(Some(TypePathContext::module(module_ref))),
            ItemOwner::Trait(id) => Ok(self
                .trait_data(TraitRef { origin, id })?
                .map(|data| TypePathContext::module(data.owner))),
            ItemOwner::Impl(id) => {
                let impl_ref = ImplRef { origin, id };
                Ok(self.impl_data(impl_ref)?.map(|data| TypePathContext {
                    module: data.owner,
                    impl_ref: Some(impl_ref),
                }))
            }
        }
    }

    pub fn semantic_item_for_local_def(
        &self,
        def: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, PackageStoreError> {
        if let Some(target) = def.origin.as_target_ref()
            && let Some(items) = self.items(target)?
            && let Some(item) = items.item_for_local_def(def.local_def)
        {
            Ok(Some(item.semantic_ref(def.origin)))
        } else {
            return Ok(None);
        }
    }

    pub fn generic_params_for_type_def(
        &self,
        ty: TypeDefRef,
    ) -> Result<Option<&rg_item_tree::GenericParams>, PackageStoreError> {
        if let Some(target) = ty.origin.as_target_ref()
            && let Some(items) = self.items(target)?
        {
            Ok(items.generic_params_for_type_def(ty.id))
        } else {
            return Ok(None);
        }
    }

    pub fn type_def_name(&self, ty: TypeDefRef) -> Result<Option<&str>, PackageStoreError> {
        if let Some(target) = ty.origin.as_target_ref()
            && let Some(items) = self.items(target)?
        {
            Ok(items.type_def_name(ty.id))
        } else {
            Ok(None)
        }
    }

    pub fn enum_data_for_type_def(
        &self,
        ty: TypeDefRef,
    ) -> Result<Option<&EnumData>, PackageStoreError> {
        if let TypeDefId::Enum(id) = ty.id
            && let Some(target) = ty.origin.as_target_ref()
            && let Some(items) = self.items(target)?
        {
            Ok(items.enum_data(id))
        } else {
            Ok(None)
        }
    }

    pub fn enum_variant_for_type_def(
        &self,
        ty: TypeDefRef,
        variant_name: &str,
    ) -> Result<Option<(usize, &rg_item_tree::EnumVariantItem)>, PackageStoreError> {
        // TODO: It would be nice to move this whole family of functions to enum data,
        // but right now enum data lacks information to create enum variant refs.
        // Moreover I'm not sure if keeping this kind of design is correct in the
        // first place.
        if let Some(data) = self.enum_data_for_type_def(ty)? {
            Ok(data
                .variants
                .iter()
                .enumerate()
                .find(|(_, variant)| variant.name == variant_name))
        } else {
            Ok(None)
        }
    }

    pub fn enum_variant_ref_for_type_def(
        &self,
        ty: TypeDefRef,
        variant_name: &str,
    ) -> Result<Option<EnumVariantRef>, PackageStoreError> {
        if let TypeDefId::Enum(enum_id) = ty.id
            && let Some((index, _)) = self.enum_variant_for_type_def(ty, variant_name)?
        {
            Ok(Some(EnumVariantRef {
                origin: ty.origin,
                enum_id,
                index,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn enum_variant_data(
        &self,
        variant_ref: EnumVariantRef,
    ) -> Result<Option<EnumVariantData<'_>>, PackageStoreError> {
        if let Some(target) = variant_ref.origin.as_target_ref()
            && let Some(items) = self.items(target)?
            && let Some(data) = items.enum_data(variant_ref.enum_id)
            && let Some(variant) = data.variants.get(variant_ref.index)
        {
            Ok(Some(EnumVariantData {
                owner: TypeDefRef {
                    origin: variant_ref.origin,
                    id: TypeDefId::Enum(variant_ref.enum_id),
                },
                owner_module: data.owner,
                file_id: data.source.file_id,
                variant,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn impl_data(&self, impl_ref: ImplRef) -> Result<Option<&ImplData>, PackageStoreError> {
        if let Some(target) = impl_ref.origin.as_target_ref()
            && let Some(items) = self.items(target)?
        {
            Ok(items.impl_data(impl_ref.id))
        } else {
            Ok(None)
        }
    }

    pub fn trait_data(&self, trait_ref: TraitRef) -> Result<Option<&TraitData>, PackageStoreError> {
        if let Some(target) = trait_ref.origin.as_target_ref()
            && let Some(items) = self.items(target)?
        {
            Ok(items.trait_data(trait_ref.id))
        } else {
            Ok(None)
        }
    }

    pub fn function_data(
        &self,
        function_ref: FunctionRef,
    ) -> Result<Option<&FunctionData>, PackageStoreError> {
        let Some(target) = function_ref.origin.as_target_ref() else {
            return Ok(None);
        };
        Ok(self
            .items(target)?
            .and_then(|items| items.function_data(function_ref.id)))
    }

    pub fn type_alias_data(
        &self,
        type_alias_ref: TypeAliasRef,
    ) -> Result<Option<&TypeAliasData>, PackageStoreError> {
        let Some(target) = type_alias_ref.origin.as_target_ref() else {
            return Ok(None);
        };
        Ok(self
            .items(target)?
            .and_then(|items| items.type_alias_data(type_alias_ref.id)))
    }

    pub fn const_data(&self, const_ref: ConstRef) -> Result<Option<&ConstData>, PackageStoreError> {
        let Some(target) = const_ref.origin.as_target_ref() else {
            return Ok(None);
        };
        Ok(self
            .items(target)?
            .and_then(|items| items.const_data(const_ref.id)))
    }

    pub fn static_data(
        &self,
        static_ref: StaticRef,
    ) -> Result<Option<&StaticData>, PackageStoreError> {
        let Some(target) = static_ref.origin.as_target_ref() else {
            return Ok(None);
        };
        Ok(self
            .items(target)?
            .and_then(|items| items.static_data(static_ref.id)))
    }

    pub fn fields_for_type(&self, ty: TypeDefRef) -> Result<Vec<FieldRef>, PackageStoreError> {
        let Some(target) = ty.origin.as_target_ref() else {
            return Ok(Vec::new());
        };
        let Some(items) = self.items(target)? else {
            return Ok(Vec::new());
        };
        let maybe_field_count = match ty.id {
            TypeDefId::Struct(id) => items.struct_data(id).map(|data| data.fields.fields().len()),
            TypeDefId::Union(id) => items.union_data(id).map(|data| data.fields.len()),
            TypeDefId::Enum(_) => None,
        };
        let Some(field_count) = maybe_field_count else {
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
        let items = if let Some(target) = field_ref.owner.origin.as_target_ref()
            && let Some(items) = self.items(target)?
        {
            items
        } else {
            return Ok(None);
        };
        let field = match field_ref.owner.id {
            TypeDefId::Struct(id) => {
                let Some(data) = items.struct_data(id) else {
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
                let Some(data) = items.union_data(id) else {
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

    pub fn impls_for_trait(&self, trait_ref: TraitRef) -> Result<Vec<ImplRef>, PackageStoreError> {
        let mut impls = Vec::new();

        for impl_ref in self.impl_refs()? {
            let Some(data) = self.impl_data(impl_ref)? else {
                continue;
            };
            if data.resolved_trait_refs.contains(&trait_ref) {
                push_unique(&mut impls, impl_ref);
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

    pub fn inherent_functions_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<Vec<FunctionRef>, PackageStoreError> {
        let mut functions = Vec::new();

        for impl_ref in self.inherent_impls_for_type(ty)? {
            let Some(data) = self.impl_data(impl_ref)? else {
                continue;
            };

            functions.extend(data.functions());
        }

        Ok(functions)
    }

    pub fn trait_functions_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<Vec<FunctionRef>, PackageStoreError> {
        let mut functions = Vec::new();

        for trait_ref in self.traits_for_type(ty)? {
            let data = self
                .trait_data(trait_ref)?
                .expect("Trait ref returned by own method");
            for function in data.functions() {
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
            functions.extend(data.functions());
        }

        Ok(functions)
    }

    fn impl_refs(&self) -> Result<Vec<ImplRef>, PackageStoreError> {
        let mut impl_refs = Vec::new();

        for store in self.included_stores()? {
            impl_refs.extend(store.impls_with_refs().map(|(impl_ref, _)| impl_ref));
        }

        Ok(impl_refs)
    }
}
