//! Shared semantic operations over borrowed declarations.

use rg_def_map::LocalEnumVariantData;
use rg_ir_model::{
    AssocItemId, ConstRef, DefMapRef, EnumVariantFieldRef, EnumVariantRef, FieldKey, FieldRef,
    FunctionRef, GenericDefRef, ImplRef, ItemOwner, LocalDefRef, LocalEnumVariantRef, ModuleRef,
    SemanticItemRef, StaticRef, TraitDefRef, TypeAliasRef, TypeDefId, TypeDefRef,
};
use rg_item_tree::FieldList;
use rg_source::ReadError;

use super::SemanticItemSource;
use crate::{
    ConstData, EnumData, EnumVariantData, FieldData, FunctionData, ImplData, SelfTypeOwner,
    SemanticItemView, StaticData, TraitData, TypeAliasData, TypePathContext,
};

/// Shared item operations, independent of how providers acquire declaration data.
#[derive(Clone)]
pub struct ItemStoreQuery<'a, S> {
    source: S,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a, S> ItemStoreQuery<'a, S>
where
    S: SemanticItemSource<'a>,
{
    /// Wraps a declaration source with the common item query API.
    pub fn new(source: S) -> Self {
        Self {
            source,
            _marker: std::marker::PhantomData,
        }
    }

    /// Enumerates declarations explicitly; ordinary item reads do not require this scan.
    pub fn semantic_items_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Vec<SemanticItemView<'a>>, ReadError> {
        self.source.all_items(origin)
    }

    pub fn semantic_item_view(
        &self,
        item: SemanticItemRef,
    ) -> Result<Option<SemanticItemView<'a>>, ReadError> {
        self.source.item(item)
    }

    pub fn semantic_item_for_local_def(
        &self,
        local_def: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, ReadError> {
        self.source.item_for_local_def(local_def)
    }

    /// Recovers the type-path context needed to resolve types inside a function signature/body.
    pub fn type_path_context_for_function(
        &self,
        function_ref: FunctionRef,
    ) -> Result<Option<TypePathContext>, ReadError> {
        let Some(function_data) = self.function_data(function_ref)? else {
            return Ok(None);
        };
        self.type_path_context_for_owner(function_ref.origin, function_data.owner)
    }

    /// Recovers the definition context in which generic defaults and predicates were written.
    pub fn type_path_context_for_generic_def(
        &self,
        owner: GenericDefRef,
    ) -> Result<Option<TypePathContext>, ReadError> {
        match owner {
            GenericDefRef::TypeDef(type_def) => {
                Ok(self
                    .type_def_owner(type_def)?
                    .map(|module| TypePathContext {
                        module,
                        self_owner: Some(SelfTypeOwner::TypeDef(type_def)),
                    }))
            }
            GenericDefRef::Trait(trait_ref) => {
                Ok(self.trait_data(trait_ref)?.map(|data| TypePathContext {
                    module: data.owner,
                    self_owner: Some(SelfTypeOwner::Trait(trait_ref)),
                }))
            }
            GenericDefRef::Impl(impl_ref) => {
                Ok(self.impl_data(impl_ref)?.map(|data| TypePathContext {
                    module: data.owner,
                    self_owner: Some(SelfTypeOwner::Impl(impl_ref)),
                }))
            }
            GenericDefRef::Function(function) => self.type_path_context_for_function(function),
            GenericDefRef::TypeAlias(alias) => {
                let Some(data) = self.type_alias_data(alias)? else {
                    return Ok(None);
                };
                self.type_path_context_for_owner(alias.origin, data.owner)
            }
            GenericDefRef::Const(konst) => {
                let Some(data) = self.const_data(konst)? else {
                    return Ok(None);
                };
                self.type_path_context_for_owner(konst.origin, data.owner)
            }
            GenericDefRef::Static(static_ref) => {
                let Some(data) = self.static_data(static_ref)? else {
                    return Ok(None);
                };
                Ok(Some(TypePathContext::module(data.owner)))
            }
        }
    }

    /// Builds the type-path context for an item owner without exposing owner-specific lookup code.
    pub fn type_path_context_for_owner(
        &self,
        origin: DefMapRef,
        owner: ItemOwner,
    ) -> Result<Option<TypePathContext>, ReadError> {
        match owner {
            ItemOwner::Module(module) => Ok(Some(TypePathContext::module(module))),
            ItemOwner::Trait(id) => {
                let trait_ref = TraitDefRef { origin, id };
                Ok(self.trait_data(trait_ref)?.map(|data| TypePathContext {
                    module: data.owner,
                    self_owner: Some(SelfTypeOwner::Trait(trait_ref)),
                }))
            }
            ItemOwner::Impl(id) => {
                let impl_ref = ImplRef { origin, id };
                Ok(self.impl_data(impl_ref)?.map(|data| TypePathContext {
                    module: data.owner,
                    self_owner: Some(SelfTypeOwner::Impl(impl_ref)),
                }))
            }
        }
    }

    /// Reads the owner module for a nominal type.
    pub fn type_def_owner(&self, ty: TypeDefRef) -> Result<Option<ModuleRef>, ReadError> {
        Ok(self
            .source
            .item(ty.into())?
            .and_then(SemanticItemView::module_owner))
    }

    /// Returns the display name for a nominal type ref, which intentionally carries only an ID.
    pub fn type_def_name(&self, ty: TypeDefRef) -> Result<Option<&'a str>, ReadError> {
        Ok(self
            .source
            .item(ty.into())?
            .and_then(SemanticItemView::name)
            .map(|name| name.as_str()))
    }

    /// Return the source local def for a nominal type, if its item data is available.
    pub fn local_def_for_type_def(&self, ty: TypeDefRef) -> Result<Option<LocalDefRef>, ReadError> {
        Ok(self
            .source
            .item(ty.into())?
            .and_then(SemanticItemView::local_def))
    }

    /// Keeps the struct-constructor rule next to item data instead of duplicating shape checks in
    /// value lookup.
    pub fn type_def_has_value_constructor(&self, ty: TypeDefRef) -> Result<bool, ReadError> {
        Ok(self
            .source
            .item(ty.into())?
            .and_then(SemanticItemView::struct_data)
            .is_some_and(|data| data.fields.has_value_constructor()))
    }

    /// Lets callers that resolved through the type namespace enter enum-specific data.
    pub fn enum_data_for_type_def(
        &self,
        ty: TypeDefRef,
    ) -> Result<Option<&'a EnumData>, ReadError> {
        if !matches!(ty.id, TypeDefId::Enum(_)) {
            return Ok(None);
        }
        Ok(self
            .source
            .item(ty.into())?
            .and_then(SemanticItemView::enum_data))
    }

    /// Maps a variant name from syntax/resolution to the stable variant ref used by analysis.
    pub fn enum_variant_ref_for_type_def(
        &self,
        ty: TypeDefRef,
        variant_name: &str,
    ) -> Result<Option<EnumVariantRef>, ReadError> {
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

    /// Projects an enum local-def plus variant slot into the semantic variant ref lowered for it.
    pub fn enum_variant_ref_for_local_def_index(
        &self,
        enum_def: LocalDefRef,
        index: usize,
        expected_name: Option<&str>,
    ) -> Result<Option<EnumVariantRef>, ReadError> {
        if let Some(SemanticItemRef::TypeDef(TypeDefRef {
            origin,
            id: TypeDefId::Enum(enum_id),
        })) = self.semantic_item_for_local_def(enum_def)?
            && let Some(data) = self.enum_data_for_type_def(TypeDefRef {
                origin,
                id: TypeDefId::Enum(enum_id),
            })?
            && let Some(variant) = data.variants.get(index)
            && expected_name.is_none_or(|expected_name| variant.name == expected_name)
        {
            Ok(Some(EnumVariantRef {
                origin,
                enum_id,
                index,
            }))
        } else {
            Ok(None)
        }
    }

    /// Projects a DefMap-local enum variant into the semantic variant ref lowered for it.
    pub fn enum_variant_ref_for_local_enum_variant(
        &self,
        variant_ref: LocalEnumVariantRef,
        variant_data: &LocalEnumVariantData,
    ) -> Result<Option<EnumVariantRef>, ReadError> {
        self.enum_variant_ref_for_local_def_index(
            LocalDefRef {
                origin: variant_ref.origin,
                local_def: variant_data.enum_def,
            },
            variant_data.index,
            Some(variant_data.name.as_str()),
        )
    }

    /// Expands a variant ref with its enum owner and source facts.
    pub fn enum_variant_data(
        &self,
        variant_ref: EnumVariantRef,
    ) -> Result<Option<EnumVariantData<'a>>, ReadError> {
        if let Some(data) = self.enum_data_for_type_def(TypeDefRef {
            origin: variant_ref.origin,
            id: TypeDefId::Enum(variant_ref.enum_id),
        })? && let Some(variant) = data.variants.get(variant_ref.index)
        {
            Ok(Some(EnumVariantData {
                owner: TypeDefRef {
                    origin: variant_ref.origin,
                    id: TypeDefId::Enum(variant_ref.enum_id),
                },
                owner_module: data.owner,
                source: data.source,
                variant,
            }))
        } else {
            Ok(None)
        }
    }

    /// Enumerates stable field refs below one enum variant.
    pub fn fields_for_enum_variant(
        &self,
        variant: EnumVariantRef,
    ) -> Result<Vec<EnumVariantFieldRef>, ReadError> {
        let Some(data) = self.enum_variant_data(variant)? else {
            return Ok(Vec::new());
        };
        Ok((0..data.variant.fields.fields().len())
            .map(|index| EnumVariantFieldRef {
                owner: variant,
                index,
            })
            .collect())
    }

    /// Expands a variant-field ref with the source facts shared by ordinary member fields.
    pub fn enum_variant_field_data(
        &self,
        field_ref: EnumVariantFieldRef,
    ) -> Result<Option<FieldData<'a>>, ReadError> {
        let Some(data) = self.enum_variant_data(field_ref.owner)? else {
            return Ok(None);
        };
        let Some(field) = data.variant.fields.fields().get(field_ref.index) else {
            return Ok(None);
        };
        Ok(Some(FieldData {
            owner_module: data.owner_module,
            source: data.source,
            field,
        }))
    }

    /// Exposes the constructor field shape for a nominal struct.
    ///
    /// Enums need a selected variant and unions are not pattern constructors, so neither is
    /// flattened into this query.
    pub fn struct_fields_for_type_def(
        &self,
        ty: TypeDefRef,
    ) -> Result<Option<&'a FieldList>, ReadError> {
        if !matches!(ty.id, TypeDefId::Struct(_)) {
            return Ok(None);
        }
        Ok(self
            .source
            .item(ty.into())?
            .and_then(SemanticItemView::struct_data)
            .map(|data| &data.fields))
    }

    /// Enumerates stable field refs without exposing whether the owner stores struct or union
    /// fields.
    pub fn fields_for_type(&self, ty: TypeDefRef) -> Result<Vec<FieldRef>, ReadError> {
        let Some(item) = self.source.item(ty.into())? else {
            return Ok(Vec::new());
        };
        let field_count = item
            .struct_data()
            .map(|data| data.fields.fields().len())
            .or_else(|| item.union_data().map(|data| data.fields.len()))
            .unwrap_or(0);
        Ok((0..field_count)
            .map(|index| FieldRef { owner: ty, index })
            .collect())
    }

    /// Maps a syntax-level field key, named or tuple, to the stable field ref.
    pub fn field_for_type(
        &self,
        ty: TypeDefRef,
        key: &FieldKey,
    ) -> Result<Option<FieldRef>, ReadError> {
        let Some(item) = self.source.item(ty.into())? else {
            return Ok(None);
        };
        let fields = item
            .struct_data()
            .map(|data| data.fields.fields())
            .or_else(|| item.union_data().map(|data| data.fields.as_slice()));
        let index = fields.and_then(|fields| match key {
            FieldKey::Named(_) => fields
                .iter()
                .position(|field| field.key.as_ref() == Some(key)),
            FieldKey::Tuple(index) => fields
                .get(*index)
                .filter(|field| field.key.as_ref() == Some(key))
                .map(|_| *index),
        });
        Ok(index.map(|index| FieldRef { owner: ty, index }))
    }

    /// Expands a field ref with the owner/source context that member views need.
    pub fn field_data(&self, field_ref: FieldRef) -> Result<Option<FieldData<'a>>, ReadError> {
        let Some(item) = self.source.item(field_ref.owner.into())? else {
            return Ok(None);
        };
        let (owner_module, source, fields) = if let Some(data) = item.struct_data() {
            (data.owner, data.source, data.fields.fields())
        } else if let Some(data) = item.union_data() {
            (data.owner, data.source, data.fields.as_slice())
        } else {
            return Ok(None);
        };
        Ok(fields.get(field_ref.index).map(|field| FieldData {
            owner_module,
            source,
            field,
        }))
    }

    /// Provides trait signature/docs/associated-item data after resolution has picked a trait.
    pub fn trait_data(&self, trait_ref: TraitDefRef) -> Result<Option<&'a TraitData>, ReadError> {
        Ok(self
            .source
            .item(trait_ref.into())?
            .and_then(SemanticItemView::trait_data))
    }

    /// Find an associated type declared directly by this trait.
    ///
    /// Supertrait traversal belongs to type lowering because it needs generic and path context.
    /// This item-shaped operation only follows the trait's own associated-item IDs.
    pub fn declared_associated_type_by_name(
        &self,
        trait_ref: TraitDefRef,
        name: &str,
    ) -> Result<Option<TypeAliasRef>, ReadError> {
        let Some(data) = self.trait_data(trait_ref)? else {
            return Ok(None);
        };
        self.associated_type_by_name(trait_ref.origin, &data.items, name)
    }

    /// Follows an impl ref into the header and associated items used by member/type queries.
    pub fn impl_data(&self, impl_ref: ImplRef) -> Result<Option<&'a ImplData>, ReadError> {
        Ok(self
            .source
            .item(impl_ref.into())?
            .and_then(SemanticItemView::impl_data))
    }

    /// Find an associated type declared directly by this impl.
    ///
    /// Trait association is name-based in Rust, so both program materialization and direct
    /// projection use this query instead of independently walking an impl's item IDs.
    pub fn impl_associated_type_by_name(
        &self,
        impl_ref: ImplRef,
        name: &str,
    ) -> Result<Option<TypeAliasRef>, ReadError> {
        let Some(data) = self.impl_data(impl_ref)? else {
            return Ok(None);
        };
        self.associated_type_by_name(impl_ref.origin, &data.items, name)
    }

    /// Find a named type alias inside either kind of associated-item list.
    fn associated_type_by_name(
        &self,
        origin: DefMapRef,
        items: &[AssocItemId],
        name: &str,
    ) -> Result<Option<TypeAliasRef>, ReadError> {
        for item in items {
            let AssocItemId::TypeAlias(id) = item else {
                continue;
            };
            let alias = TypeAliasRef { origin, id: *id };
            if self
                .type_alias_data(alias)?
                .is_some_and(|data| data.name.as_str() == name)
            {
                return Ok(Some(alias));
            }
        }
        Ok(None)
    }

    /// Provides lowered function facts for both free functions and associated functions.
    pub fn function_data(
        &self,
        function_ref: FunctionRef,
    ) -> Result<Option<&'a FunctionData>, ReadError> {
        Ok(self
            .source
            .item(function_ref.into())?
            .and_then(SemanticItemView::function_data))
    }

    /// Lets type projection inspect an alias after name resolution selected it.
    pub fn type_alias_data(
        &self,
        type_alias_ref: TypeAliasRef,
    ) -> Result<Option<&'a TypeAliasData>, ReadError> {
        Ok(self
            .source
            .item(type_alias_ref.into())?
            .and_then(SemanticItemView::type_alias_data))
    }

    /// Provides const item facts for hover, definition, and type projection.
    pub fn const_data(&self, const_ref: ConstRef) -> Result<Option<&'a ConstData>, ReadError> {
        Ok(self
            .source
            .item(const_ref.into())?
            .and_then(SemanticItemView::const_data))
    }

    /// Provides static item facts for hover, definition, and type projection.
    pub fn static_data(&self, static_ref: StaticRef) -> Result<Option<&'a StaticData>, ReadError> {
        Ok(self
            .source
            .item(static_ref.into())?
            .and_then(SemanticItemView::static_data))
    }
}
