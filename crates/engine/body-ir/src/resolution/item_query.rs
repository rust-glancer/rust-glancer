//! Body-aware access to semantic-shaped item storage.
//!
//! Body resolution is being migrated toward semantic item identities for body-local declarations.
//! This query object keeps that storage routing in one place: target-origin refs are answered by
//! Semantic IR, and refs for the body currently being resolved are answered by the shadow item
//! store collected for that body.

use rg_ir_model::{
    ConstRef, DefMapRef, EnumVariantRef, FieldRef, FunctionRef, ImplRef, ItemOwner, LocalDefRef,
    SemanticItemRef, StaticRef, TargetRef, TraitRef, TypeAliasRef, TypeDefId, TypeDefRef,
    hir::items::{
        ConstData, EnumData, EnumVariantData, FieldData, FunctionData, ImplData, StaticData,
        TraitData, TypeAliasData,
    },
};
use rg_item_tree::{FieldKey, FieldList};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemStore, SemanticIrReadTxn, TypePathContext};

use crate::ir::body::BodyData;

pub(super) struct BodyItemQuery<'query, 'db, 'body> {
    semantic_ir: &'query SemanticIrReadTxn<'db>,
    body_ref: rg_ir_model::BodyRef,
    body: &'body BodyData,
}

impl<'query, 'db, 'body> BodyItemQuery<'query, 'db, 'body> {
    pub(super) fn new(
        semantic_ir: &'query SemanticIrReadTxn<'db>,
        body_ref: rg_ir_model::BodyRef,
        body: &'body BodyData,
    ) -> Self {
        Self {
            semantic_ir,
            body_ref,
            body,
        }
    }

    pub(super) fn semantic_item_for_local_def(
        &self,
        def: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, PackageStoreError> {
        if def.origin.as_target_ref().is_some() {
            return self.semantic_ir.semantic_item_for_local_def(def);
        }

        Ok(self
            .item_store_for_origin(def.origin)?
            .and_then(|items| items.item_for_local_def(def.local_def))
            .map(|item| item.semantic_ref(def.origin)))
    }

    pub(super) fn type_path_context_for_function(
        &self,
        function_ref: FunctionRef,
    ) -> Result<Option<TypePathContext>, PackageStoreError> {
        if function_ref.origin.as_target_ref().is_some() {
            return self
                .semantic_ir
                .type_path_context_for_function(function_ref);
        }

        let Some(function_data) = self.function_data(function_ref)? else {
            return Ok(None);
        };
        self.type_path_context_for_owner(function_ref.origin, function_data.owner)
    }

    pub(super) fn generic_params_for_type_def(
        &self,
        ty: TypeDefRef,
    ) -> Result<Option<&rg_item_tree::GenericParams>, PackageStoreError> {
        if ty.origin.as_target_ref().is_some() {
            return self.semantic_ir.generic_params_for_type_def(ty);
        }

        Ok(self
            .item_store_for_origin(ty.origin)?
            .and_then(|items| items.generic_params_for_type_def(ty.id)))
    }

    pub(super) fn type_def_name(&self, ty: TypeDefRef) -> Result<Option<&str>, PackageStoreError> {
        if ty.origin.as_target_ref().is_some() {
            return self.semantic_ir.type_def_name(ty);
        }

        Ok(self
            .item_store_for_origin(ty.origin)?
            .and_then(|items| items.type_def_name(ty.id)))
    }

    pub(super) fn type_def_has_value_constructor(
        &self,
        ty: TypeDefRef,
    ) -> Result<bool, PackageStoreError> {
        let Some(items) = self.item_store_for_origin(ty.origin)? else {
            return Ok(false);
        };

        Ok(match ty.id {
            TypeDefId::Struct(id) => items
                .struct_data(id)
                .is_some_and(|data| matches!(data.fields, FieldList::Tuple(_) | FieldList::Unit)),
            TypeDefId::Enum(_) | TypeDefId::Union(_) => false,
        })
    }

    pub(super) fn enum_data_for_type_def(
        &self,
        ty: TypeDefRef,
    ) -> Result<Option<&EnumData>, PackageStoreError> {
        if ty.origin.as_target_ref().is_some() {
            return self.semantic_ir.enum_data_for_type_def(ty);
        }

        let TypeDefId::Enum(id) = ty.id else {
            return Ok(None);
        };
        Ok(self
            .item_store_for_origin(ty.origin)?
            .and_then(|items| items.enum_data(id)))
    }

    pub(super) fn enum_variant_ref_for_type_def(
        &self,
        ty: TypeDefRef,
        variant_name: &str,
    ) -> Result<Option<EnumVariantRef>, PackageStoreError> {
        if ty.origin.as_target_ref().is_some() {
            return self
                .semantic_ir
                .enum_variant_ref_for_type_def(ty, variant_name);
        }

        let TypeDefId::Enum(enum_id) = ty.id else {
            return Ok(None);
        };
        Ok(self
            .enum_data_for_type_def(ty)?
            .and_then(|data| {
                data.variants
                    .iter()
                    .position(|variant| variant.name == variant_name)
            })
            .map(|index| EnumVariantRef {
                origin: ty.origin,
                enum_id,
                index,
            }))
    }

    pub(super) fn enum_variant_data(
        &self,
        variant_ref: EnumVariantRef,
    ) -> Result<Option<EnumVariantData<'_>>, PackageStoreError> {
        if variant_ref.origin.as_target_ref().is_some() {
            return self.semantic_ir.enum_variant_data(variant_ref);
        }

        let Some(items) = self.item_store_for_origin(variant_ref.origin)? else {
            return Ok(None);
        };
        let Some(data) = items.enum_data(variant_ref.enum_id) else {
            return Ok(None);
        };
        let Some(variant) = data.variants.get(variant_ref.index) else {
            return Ok(None);
        };

        Ok(Some(EnumVariantData {
            owner: TypeDefRef {
                origin: variant_ref.origin,
                id: TypeDefId::Enum(variant_ref.enum_id),
            },
            owner_module: data.owner,
            file_id: data.source.file_id,
            variant,
        }))
    }

    pub(super) fn field_for_type(
        &self,
        ty: TypeDefRef,
        key: &FieldKey,
    ) -> Result<Option<FieldRef>, PackageStoreError> {
        if ty.origin.as_target_ref().is_some() {
            return self.semantic_ir.field_for_type(ty, key);
        }

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

    pub(super) fn field_data(
        &self,
        field_ref: FieldRef,
    ) -> Result<Option<FieldData<'_>>, PackageStoreError> {
        if field_ref.owner.origin.as_target_ref().is_some() {
            return self.semantic_ir.field_data(field_ref);
        }

        let Some(items) = self.item_store_for_origin(field_ref.owner.origin)? else {
            return Ok(None);
        };
        let data = match field_ref.owner.id {
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

        Ok(Some(data))
    }

    pub(super) fn trait_data(
        &self,
        trait_ref: TraitRef,
    ) -> Result<Option<&TraitData>, PackageStoreError> {
        if trait_ref.origin.as_target_ref().is_some() {
            return self.semantic_ir.trait_data(trait_ref);
        }

        Ok(self
            .item_store_for_origin(trait_ref.origin)?
            .and_then(|items| items.trait_data(trait_ref.id)))
    }

    pub(super) fn impl_data(
        &self,
        impl_ref: ImplRef,
    ) -> Result<Option<&ImplData>, PackageStoreError> {
        if impl_ref.origin.as_target_ref().is_some() {
            return self.semantic_ir.impl_data(impl_ref);
        }

        Ok(self
            .item_store_for_origin(impl_ref.origin)?
            .and_then(|items| items.impl_data(impl_ref.id)))
    }

    pub(super) fn function_data(
        &self,
        function_ref: FunctionRef,
    ) -> Result<Option<&FunctionData>, PackageStoreError> {
        if function_ref.origin.as_target_ref().is_some() {
            return self.semantic_ir.function_data(function_ref);
        }

        Ok(self
            .item_store_for_origin(function_ref.origin)?
            .and_then(|items| items.function_data(function_ref.id)))
    }

    pub(super) fn type_alias_data(
        &self,
        type_alias_ref: TypeAliasRef,
    ) -> Result<Option<&TypeAliasData>, PackageStoreError> {
        if type_alias_ref.origin.as_target_ref().is_some() {
            return self.semantic_ir.type_alias_data(type_alias_ref);
        }

        Ok(self
            .item_store_for_origin(type_alias_ref.origin)?
            .and_then(|items| items.type_alias_data(type_alias_ref.id)))
    }

    pub(super) fn const_data(
        &self,
        const_ref: ConstRef,
    ) -> Result<Option<&ConstData>, PackageStoreError> {
        if const_ref.origin.as_target_ref().is_some() {
            return self.semantic_ir.const_data(const_ref);
        }

        Ok(self
            .item_store_for_origin(const_ref.origin)?
            .and_then(|items| items.const_data(const_ref.id)))
    }

    pub(super) fn static_data(
        &self,
        static_ref: StaticRef,
    ) -> Result<Option<&StaticData>, PackageStoreError> {
        if static_ref.origin.as_target_ref().is_some() {
            return self.semantic_ir.static_data(static_ref);
        }

        Ok(self
            .item_store_for_origin(static_ref.origin)?
            .and_then(|items| items.static_data(static_ref.id)))
    }

    pub(super) fn inherent_impls_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<Vec<ImplRef>, PackageStoreError> {
        if ty.origin.as_target_ref().is_some() {
            return self.semantic_ir.inherent_impls_for_type(ty);
        }

        let Some(items) = self.item_store_for_origin(ty.origin)? else {
            return Ok(Vec::new());
        };

        Ok(items
            .impls_with_refs()
            .filter_map(|(impl_ref, data)| {
                (data.trait_ref.is_none() && data.resolved_self_tys.contains(&ty))
                    .then_some(impl_ref)
            })
            .collect())
    }

    pub(super) fn inherent_functions_for_type(
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

    pub(super) fn type_path_context_for_owner(
        &self,
        origin: DefMapRef,
        owner: ItemOwner,
    ) -> Result<Option<TypePathContext>, PackageStoreError> {
        if origin.as_target_ref().is_some() {
            return self.semantic_ir.type_path_context_for_owner(origin, owner);
        }

        match owner {
            ItemOwner::Module(module) => Ok(Some(TypePathContext::module(module))),
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

    fn fields_for_type(&self, ty: TypeDefRef) -> Result<Vec<FieldRef>, PackageStoreError> {
        if ty.origin.as_target_ref().is_some() {
            return self.semantic_ir.fields_for_type(ty);
        }

        let Some(items) = self.item_store_for_origin(ty.origin)? else {
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

        Ok((0..field_count)
            .map(|index| FieldRef { owner: ty, index })
            .collect())
    }

    fn item_store_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<&ItemStore>, PackageStoreError> {
        match origin {
            DefMapRef::Target(target) => self.target_item_store(target),
            DefMapRef::Body(body_ref) if body_ref == self.body_ref => {
                Ok(self.body.body_item_store())
            }
            DefMapRef::Body(_) => Ok(None),
        }
    }

    fn target_item_store(
        &self,
        target: TargetRef,
    ) -> Result<Option<&ItemStore>, PackageStoreError> {
        self.semantic_ir.items(target)
    }
}
