//! Shared queries over semantic-shaped item stores.
//!
//! Crate and body IR store item data in the same `ItemStore` shape. This layer owns the
//! item-shaped queries, while callers provide only the origin-to-store routing policy.

use rg_def_map::LocalEnumVariantData;
use rg_ir_model::{
    AssocItemId, ConstRef, CrateRef, DefMapRef, EnumVariantFieldRef, EnumVariantRef, FieldKey,
    FieldRef, FunctionRef, GenericDefRef, ImplRef, ItemOwner, LocalDefRef, LocalEnumVariantRef,
    ModuleRef, SemanticItemRef, StaticRef, TraitDefRef, TypeAliasRef, TypeDefId, TypeDefRef,
};
use rg_item_tree::{FieldList, GenericParams};

use super::ItemStoreSource;
use crate::{
    ConstData, EnumData, EnumVariantData, FieldData, FunctionData, ImplData, ItemStore,
    SemanticItemView, StaticData, TraitData, TypeAliasData, TypePathContext,
};

/// Shared item queries over any storage that can route `DefMapRef` origins to item stores.
///
/// This keeps presentation, type, and body-resolution code from re-implementing the same
/// "find the right store, then read item data" sequence for every item kind.
#[derive(Clone)]
pub struct ItemStoreQuery<'a, S> {
    source: S,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a, S> ItemStoreQuery<'a, S>
where
    S: ItemStoreSource<'a>,
{
    /// Wraps a layer-specific store source with the common item query API.
    pub fn new(source: S) -> Self {
        Self {
            source,
            _marker: std::marker::PhantomData,
        }
    }

    /// Exposes the underlying routing for callers that need a store-level operation directly.
    pub fn item_store_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<&'a ItemStore>, S::Error> {
        self.source.item_store_for_origin(origin)
    }

    /// Returns the stores that are materialized by this query source.
    pub fn included_stores(&self) -> Result<Vec<&'a ItemStore>, S::Error> {
        self.source.included_stores()
    }

    /// Returns crate refs for all stores materialized by this query source.
    pub fn included_crate_refs(&self) -> Result<Vec<CrateRef>, S::Error> {
        Ok(self
            .included_stores()?
            .into_iter()
            .map(|store| store.crate_ref())
            .collect())
    }

    /// Returns stores for the exact crates selected by a language-visibility query.
    pub fn stores_for_crates(&self, crates: &[CrateRef]) -> Result<Vec<&'a ItemStore>, S::Error> {
        let mut stores = Vec::new();
        for crate_ref in crates {
            if let Some(store) = self.item_store_for_origin(DefMapRef::Crate(*crate_ref))? {
                stores.push(store);
            }
        }
        Ok(stores)
    }

    /// Enumerates item views from one routed origin without exposing store iteration to callers.
    pub fn semantic_items_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Vec<SemanticItemView<'a>>, S::Error> {
        Ok(self
            .item_store_for_origin(origin)?
            .map(|items| items.semantic_items().collect())
            .unwrap_or_default())
    }

    /// Expands a stable item ref into the borrowed item data used by view/projection code.
    pub fn semantic_item_view(
        &self,
        item: SemanticItemRef,
    ) -> Result<Option<SemanticItemView<'a>>, S::Error> {
        Ok(self
            .item_store_for_origin(item.origin())?
            .and_then(|items| items.semantic_item_view(item)))
    }

    /// Normalizes a DefMap local definition into the item ref produced by item-store lowering.
    pub fn semantic_item_for_local_def(
        &self,
        local_def: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, S::Error> {
        Ok(self
            .item_store_for_origin(local_def.origin)?
            .and_then(|items| items.item_for_local_def(local_def.local_def))
            .map(|item| item.semantic_ref(local_def.origin)))
    }

    /// Recovers the type-path context needed to resolve types inside a function signature/body.
    pub fn type_path_context_for_function(
        &self,
        function_ref: FunctionRef,
    ) -> Result<Option<TypePathContext>, S::Error> {
        let Some(function_data) = self.function_data(function_ref)? else {
            return Ok(None);
        };
        self.type_path_context_for_owner(function_ref.origin, function_data.owner)
    }

    /// Recovers the definition context in which generic defaults and predicates were written.
    pub fn type_path_context_for_generic_def(
        &self,
        owner: GenericDefRef,
    ) -> Result<Option<TypePathContext>, S::Error> {
        match owner {
            GenericDefRef::TypeDef(type_def) => {
                Ok(self.type_def_owner(type_def)?.map(TypePathContext::module))
            }
            GenericDefRef::Trait(trait_ref) => Ok(self
                .trait_data(trait_ref)?
                .map(|data| TypePathContext::module(data.owner))),
            GenericDefRef::Impl(impl_ref) => {
                Ok(self.impl_data(impl_ref)?.map(|data| TypePathContext {
                    module: data.owner,
                    impl_ref: Some(impl_ref),
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
    ) -> Result<Option<TypePathContext>, S::Error> {
        match owner {
            ItemOwner::Module(module) => Ok(Some(TypePathContext::module(module))),
            ItemOwner::Trait(id) => Ok(self
                .trait_data(TraitDefRef { origin, id })?
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

    /// Reads generic parameters for a nominal type from the store that owns its item data.
    pub fn generic_params_for_type_def(
        &self,
        ty: TypeDefRef,
    ) -> Result<Option<&'a GenericParams>, S::Error> {
        Ok(self
            .item_store_for_origin(ty.origin)?
            .and_then(|items| items.generic_params_for_type_def(ty.id)))
    }

    /// Reads the owner module for a nominal type from the store that owns its item data.
    pub fn type_def_owner(&self, ty: TypeDefRef) -> Result<Option<ModuleRef>, S::Error> {
        let Some(items) = self.item_store_for_origin(ty.origin)? else {
            return Ok(None);
        };

        Ok(match ty.id {
            TypeDefId::Struct(id) => items.struct_data(id).map(|data| data.owner),
            TypeDefId::Enum(id) => items.enum_data(id).map(|data| data.owner),
            TypeDefId::Union(id) => items.union_data(id).map(|data| data.owner),
        })
    }

    /// Returns the display name for a nominal type ref, which intentionally carries only an ID.
    pub fn type_def_name(&self, ty: TypeDefRef) -> Result<Option<&'a str>, S::Error> {
        Ok(self
            .item_store_for_origin(ty.origin)?
            .and_then(|items| items.type_def_name(ty.id)))
    }

    /// Return the source local def for a nominal type, if its item data is available.
    pub fn local_def_for_type_def(&self, ty: TypeDefRef) -> Result<Option<LocalDefRef>, S::Error> {
        let Some(items) = self.item_store_for_origin(ty.origin)? else {
            return Ok(None);
        };

        Ok(match ty.id {
            TypeDefId::Struct(id) => items.struct_data(id).map(|data| data.local_def),
            TypeDefId::Enum(id) => items.enum_data(id).map(|data| data.local_def),
            TypeDefId::Union(id) => items.union_data(id).map(|data| data.local_def),
        })
    }

    /// Keeps the struct-constructor rule next to item data instead of duplicating shape checks in
    /// value lookup.
    pub fn type_def_has_value_constructor(&self, ty: TypeDefRef) -> Result<bool, S::Error> {
        let Some(items) = self.item_store_for_origin(ty.origin)? else {
            return Ok(false);
        };

        Ok(match ty.id {
            TypeDefId::Struct(id) => items
                .struct_data(id)
                .is_some_and(|data| data.fields.has_value_constructor()),
            TypeDefId::Enum(_) | TypeDefId::Union(_) => false,
        })
    }

    /// Lets callers that resolved through the type namespace enter enum-specific data.
    pub fn enum_data_for_type_def(&self, ty: TypeDefRef) -> Result<Option<&'a EnumData>, S::Error> {
        let TypeDefId::Enum(id) = ty.id else {
            return Ok(None);
        };
        Ok(self
            .item_store_for_origin(ty.origin)?
            .and_then(|items| items.enum_data(id)))
    }

    /// Maps a variant name from syntax/resolution to the stable variant ref used by analysis.
    pub fn enum_variant_ref_for_type_def(
        &self,
        ty: TypeDefRef,
        variant_name: &str,
    ) -> Result<Option<EnumVariantRef>, S::Error> {
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
    ) -> Result<Option<EnumVariantRef>, S::Error> {
        if let Some(SemanticItemRef::TypeDef(TypeDefRef {
            origin,
            id: TypeDefId::Enum(enum_id),
        })) = self.semantic_item_for_local_def(enum_def)?
            && let Some(items) = self.item_store_for_origin(origin)?
            && let Some(data) = items.enum_data(enum_id)
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
    ) -> Result<Option<EnumVariantRef>, S::Error> {
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
    ) -> Result<Option<EnumVariantData<'a>>, S::Error> {
        if let Some(items) = self.item_store_for_origin(variant_ref.origin)?
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

    /// Enumerates stable field refs below one enum variant.
    pub fn fields_for_enum_variant(
        &self,
        variant: EnumVariantRef,
    ) -> Result<Vec<EnumVariantFieldRef>, S::Error> {
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
    ) -> Result<Option<FieldData<'a>>, S::Error> {
        let Some(data) = self.enum_variant_data(field_ref.owner)? else {
            return Ok(None);
        };
        let Some(field) = data.variant.fields.fields().get(field_ref.index) else {
            return Ok(None);
        };
        Ok(Some(FieldData {
            owner_module: data.owner_module,
            file_id: data.file_id,
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
    ) -> Result<Option<&'a FieldList>, S::Error> {
        let TypeDefId::Struct(id) = ty.id else {
            return Ok(None);
        };
        Ok(self
            .item_store_for_origin(ty.origin)?
            .and_then(|items| items.struct_data(id))
            .map(|data| &data.fields))
    }

    /// Enumerates stable field refs without exposing whether the owner stores struct or union
    /// fields.
    pub fn fields_for_type(&self, ty: TypeDefRef) -> Result<Vec<FieldRef>, S::Error> {
        let Some(items) = self.item_store_for_origin(ty.origin)? else {
            return Ok(Vec::new());
        };
        let Some(field_count) = (match ty.id {
            TypeDefId::Struct(id) => items.struct_data(id).map(|data| data.fields.fields().len()),
            TypeDefId::Union(id) => items.union_data(id).map(|data| data.fields.len()),
            TypeDefId::Enum(_) => None,
        }) else {
            return Ok(Vec::new());
        };

        Ok((0..field_count)
            .map(|index| FieldRef { owner: ty, index })
            .collect())
    }

    /// Maps a syntax-level field key, named or tuple, to the stable field ref.
    pub fn field_for_type(
        &self,
        ty: TypeDefRef,
        key: &FieldKey,
    ) -> Result<Option<FieldRef>, S::Error> {
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

    /// Expands a field ref with the owner/source context that member views need.
    pub fn field_data(&self, field_ref: FieldRef) -> Result<Option<FieldData<'a>>, S::Error> {
        let Some(items) = self.item_store_for_origin(field_ref.owner.origin)? else {
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

    /// Provides trait signature/docs/associated-item data after resolution has picked a trait.
    pub fn trait_data(&self, trait_ref: TraitDefRef) -> Result<Option<&'a TraitData>, S::Error> {
        Ok(self
            .item_store_for_origin(trait_ref.origin)?
            .and_then(|items| items.trait_data(trait_ref.id)))
    }

    /// Find an associated type declared directly by this trait.
    ///
    /// Supertrait traversal belongs to type lowering because it needs generic and path context.
    /// This item-shaped operation only follows the trait's own associated-item IDs.
    pub fn declared_associated_type_by_name(
        &self,
        trait_ref: TraitDefRef,
        name: &str,
    ) -> Result<Option<TypeAliasRef>, S::Error> {
        let Some(data) = self.trait_data(trait_ref)? else {
            return Ok(None);
        };
        self.associated_type_by_name(trait_ref.origin, &data.items, name)
    }

    /// Follows an impl ref into the header and associated items used by member/type queries.
    pub fn impl_data(&self, impl_ref: ImplRef) -> Result<Option<&'a ImplData>, S::Error> {
        Ok(self
            .item_store_for_origin(impl_ref.origin)?
            .and_then(|items| items.impl_data(impl_ref.id)))
    }

    /// Find an associated type declared directly by this impl.
    ///
    /// Trait association is name-based in Rust, so both program materialization and direct
    /// projection use this query instead of independently walking an impl's item IDs.
    pub fn impl_associated_type_by_name(
        &self,
        impl_ref: ImplRef,
        name: &str,
    ) -> Result<Option<TypeAliasRef>, S::Error> {
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
    ) -> Result<Option<TypeAliasRef>, S::Error> {
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
    ) -> Result<Option<&'a FunctionData>, S::Error> {
        Ok(self
            .item_store_for_origin(function_ref.origin)?
            .and_then(|items| items.function_data(function_ref.id)))
    }

    /// Lets type projection inspect an alias after name resolution selected it.
    pub fn type_alias_data(
        &self,
        type_alias_ref: TypeAliasRef,
    ) -> Result<Option<&'a TypeAliasData>, S::Error> {
        Ok(self
            .item_store_for_origin(type_alias_ref.origin)?
            .and_then(|items| items.type_alias_data(type_alias_ref.id)))
    }

    /// Provides const item facts for hover, definition, and type projection.
    pub fn const_data(&self, const_ref: ConstRef) -> Result<Option<&'a ConstData>, S::Error> {
        Ok(self
            .item_store_for_origin(const_ref.origin)?
            .and_then(|items| items.const_data(const_ref.id)))
    }

    /// Provides static item facts for hover, definition, and type projection.
    pub fn static_data(&self, static_ref: StaticRef) -> Result<Option<&'a StaticData>, S::Error> {
        Ok(self
            .item_store_for_origin(static_ref.origin)?
            .and_then(|items| items.static_data(static_ref.id)))
    }
}
