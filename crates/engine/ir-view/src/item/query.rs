//! Body-aware access to semantic-shaped item storage for view code.

use rg_ir_model::{
    ConstRef, DefMapRef, EnumVariantRef, FieldRef, FunctionRef, ImplRef, LocalDefRef,
    SemanticItemRef, StaticRef, TargetRef, TraitRef, TypeAliasRef, TypeDefId, TypeDefRef,
    hir::items::{
        ConstData, EnumData, EnumVariantData, FieldData, FunctionData, ImplData, StaticData,
        TraitData, TypeAliasData,
    },
};
use rg_semantic_ir::{FieldList, ItemStore, SemanticItemView};

use crate::IndexedViewDb;

pub(crate) struct ItemQuery<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> ItemQuery<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub(crate) fn item_store_for_origin(
        &self,
        origin: DefMapRef,
    ) -> anyhow::Result<Option<&'a ItemStore>> {
        match origin {
            DefMapRef::Target(target) => self.target_item_store(target),
            DefMapRef::Body(body_ref) => Ok(self
                .db
                .body_ir
                .body_data(body_ref)?
                .and_then(|body| body.body_item_store())),
        }
    }

    pub(crate) fn semantic_item_view(
        &self,
        item: SemanticItemRef,
    ) -> anyhow::Result<Option<SemanticItemView<'a>>> {
        Ok(self
            .item_store_for_origin(item.origin())?
            .and_then(|items| items.semantic_item_view(item)))
    }

    pub(crate) fn semantic_item_for_local_def(
        &self,
        local_def: LocalDefRef,
    ) -> anyhow::Result<Option<SemanticItemRef>> {
        if local_def.origin.as_target_ref().is_some() {
            return Ok(self.db.semantic_ir.semantic_item_for_local_def(local_def)?);
        }

        Ok(self
            .item_store_for_origin(local_def.origin)?
            .and_then(|items| items.item_for_local_def(local_def.local_def))
            .map(|item| item.semantic_ref(local_def.origin)))
    }

    pub(crate) fn type_def_has_value_constructor(&self, ty: TypeDefRef) -> anyhow::Result<bool> {
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

    pub(crate) fn enum_data_for_type_def(
        &self,
        ty: TypeDefRef,
    ) -> anyhow::Result<Option<&'a EnumData>> {
        if ty.origin.as_target_ref().is_some() {
            return Ok(self.db.semantic_ir.enum_data_for_type_def(ty)?);
        }

        let TypeDefId::Enum(id) = ty.id else {
            return Ok(None);
        };
        Ok(self
            .item_store_for_origin(ty.origin)?
            .and_then(|items| items.enum_data(id)))
    }

    pub(crate) fn enum_variant_data(
        &self,
        variant_ref: EnumVariantRef,
    ) -> anyhow::Result<Option<EnumVariantData<'a>>> {
        if variant_ref.origin.as_target_ref().is_some() {
            return Ok(self.db.semantic_ir.enum_variant_data(variant_ref)?);
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

    pub(crate) fn fields_for_type(&self, ty: TypeDefRef) -> anyhow::Result<Vec<FieldRef>> {
        if ty.origin.as_target_ref().is_some() {
            return Ok(self.db.semantic_ir.fields_for_type(ty)?);
        }

        let Some(items) = self.item_store_for_origin(ty.origin)? else {
            return Ok(Vec::new());
        };
        let field_count = match ty.id {
            TypeDefId::Struct(id) => items.struct_data(id).map(|data| data.fields.fields().len()),
            TypeDefId::Union(id) => items.union_data(id).map(|data| data.fields.len()),
            TypeDefId::Enum(_) => None,
        };
        let Some(field_count) = field_count else {
            return Ok(Vec::new());
        };

        Ok((0..field_count)
            .map(|index| FieldRef { owner: ty, index })
            .collect())
    }

    pub(crate) fn field_data(&self, field_ref: FieldRef) -> anyhow::Result<Option<FieldData<'a>>> {
        if field_ref.owner.origin.as_target_ref().is_some() {
            return Ok(self.db.semantic_ir.field_data(field_ref)?);
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

    pub(crate) fn trait_data(&self, trait_ref: TraitRef) -> anyhow::Result<Option<&'a TraitData>> {
        if trait_ref.origin.as_target_ref().is_some() {
            return Ok(self.db.semantic_ir.trait_data(trait_ref)?);
        }

        Ok(self
            .item_store_for_origin(trait_ref.origin)?
            .and_then(|items| items.trait_data(trait_ref.id)))
    }

    pub(crate) fn impl_data(&self, impl_ref: ImplRef) -> anyhow::Result<Option<&'a ImplData>> {
        if impl_ref.origin.as_target_ref().is_some() {
            return Ok(self.db.semantic_ir.impl_data(impl_ref)?);
        }

        Ok(self
            .item_store_for_origin(impl_ref.origin)?
            .and_then(|items| items.impl_data(impl_ref.id)))
    }

    pub(crate) fn function_data(
        &self,
        function_ref: FunctionRef,
    ) -> anyhow::Result<Option<&'a FunctionData>> {
        if function_ref.origin.as_target_ref().is_some() {
            return Ok(self.db.semantic_ir.function_data(function_ref)?);
        }

        Ok(self
            .item_store_for_origin(function_ref.origin)?
            .and_then(|items| items.function_data(function_ref.id)))
    }

    pub(crate) fn type_alias_data(
        &self,
        type_alias_ref: TypeAliasRef,
    ) -> anyhow::Result<Option<&'a TypeAliasData>> {
        if type_alias_ref.origin.as_target_ref().is_some() {
            return Ok(self.db.semantic_ir.type_alias_data(type_alias_ref)?);
        }

        Ok(self
            .item_store_for_origin(type_alias_ref.origin)?
            .and_then(|items| items.type_alias_data(type_alias_ref.id)))
    }

    pub(crate) fn const_data(&self, const_ref: ConstRef) -> anyhow::Result<Option<&'a ConstData>> {
        if const_ref.origin.as_target_ref().is_some() {
            return Ok(self.db.semantic_ir.const_data(const_ref)?);
        }

        Ok(self
            .item_store_for_origin(const_ref.origin)?
            .and_then(|items| items.const_data(const_ref.id)))
    }

    pub(crate) fn static_data(
        &self,
        static_ref: StaticRef,
    ) -> anyhow::Result<Option<&'a StaticData>> {
        if static_ref.origin.as_target_ref().is_some() {
            return Ok(self.db.semantic_ir.static_data(static_ref)?);
        }

        Ok(self
            .item_store_for_origin(static_ref.origin)?
            .and_then(|items| items.static_data(static_ref.id)))
    }

    pub(crate) fn impls_for_type(&self, ty: TypeDefRef) -> anyhow::Result<Vec<ImplRef>> {
        if ty.origin.as_target_ref().is_some() {
            return Ok(self.db.semantic_ir.impls_for_type(ty)?);
        }

        let Some(items) = self.item_store_for_origin(ty.origin)? else {
            return Ok(Vec::new());
        };

        Ok(items
            .impls_with_refs()
            .filter_map(|(impl_ref, data)| data.resolved_self_tys.contains(&ty).then_some(impl_ref))
            .collect())
    }

    pub(crate) fn inherent_impls_for_type(&self, ty: TypeDefRef) -> anyhow::Result<Vec<ImplRef>> {
        if ty.origin.as_target_ref().is_some() {
            return Ok(self.db.semantic_ir.inherent_impls_for_type(ty)?);
        }

        Ok(self
            .impls_for_type(ty)?
            .into_iter()
            .filter_map(|impl_ref| match self.impl_data(impl_ref) {
                Ok(Some(data)) if data.trait_ref.is_none() => Some(Ok(impl_ref)),
                Ok(_) => None,
                Err(err) => Some(Err(err)),
            })
            .collect::<anyhow::Result<Vec<_>>>()?)
    }

    pub(crate) fn inherent_functions_for_type(
        &self,
        ty: TypeDefRef,
    ) -> anyhow::Result<Vec<FunctionRef>> {
        let mut functions = Vec::new();
        for impl_ref in self.inherent_impls_for_type(ty)? {
            let Some(data) = self.impl_data(impl_ref)? else {
                continue;
            };
            functions.extend(data.functions());
        }
        Ok(functions)
    }

    pub(crate) fn impls_for_trait(&self, trait_ref: TraitRef) -> anyhow::Result<Vec<ImplRef>> {
        if trait_ref.origin.as_target_ref().is_some() {
            return Ok(self.db.semantic_ir.impls_for_trait(trait_ref)?);
        }

        let Some(items) = self.item_store_for_origin(trait_ref.origin)? else {
            return Ok(Vec::new());
        };

        Ok(items
            .impls_with_refs()
            .filter_map(|(impl_ref, data)| {
                data.resolved_trait_refs
                    .contains(&trait_ref)
                    .then_some(impl_ref)
            })
            .collect())
    }

    fn target_item_store(&self, target: TargetRef) -> anyhow::Result<Option<&'a ItemStore>> {
        Ok(self.db.semantic_ir.items(target)?)
    }
}
