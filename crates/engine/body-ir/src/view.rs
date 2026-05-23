//! Borrowed body-local declaration views shared by read transactions and downstream queries.

use rg_item_tree::TypeRef;
use rg_text::Name;

use crate::{
    BindingData, BodyEnumVariantData, BodyFieldData, BodyFunctionData, BodyFunctionOwner,
    BodyImplData, BodyItemData, BodyItemKind, BodySource, BodyValueItemData, BodyValueItemKind,
};

/// Borrowed declaration-shaped facts for one body-local declaration.
#[derive(Debug, Clone, Copy)]
pub struct BodyDeclarationView<'a> {
    data: BodyDeclarationData<'a>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum BodyDeclarationData<'a> {
    Binding(&'a BindingData),
    Item(&'a BodyItemData),
    ValueItem(&'a BodyValueItemData),
    Impl(&'a BodyImplData),
    Field(BodyFieldData<'a>),
    EnumVariant(BodyEnumVariantData<'a>),
    Function(&'a BodyFunctionData),
}

impl<'a> BodyDeclarationView<'a> {
    pub(crate) fn new(data: BodyDeclarationData<'a>) -> Self {
        Self { data }
    }

    pub fn source(self) -> BodySource {
        match self.data {
            BodyDeclarationData::Binding(data) => data.source,
            BodyDeclarationData::Item(data) => data.source,
            BodyDeclarationData::ValueItem(data) => data.source,
            BodyDeclarationData::Impl(data) => data.source,
            BodyDeclarationData::Field(data) => BodySource {
                file_id: data.item.source.file_id,
                span: data.field.span,
            },
            BodyDeclarationData::EnumVariant(data) => BodySource {
                file_id: data.item.source.file_id,
                span: data.variant.span,
            },
            BodyDeclarationData::Function(data) => data.source,
        }
    }

    pub fn name_source(self) -> Option<BodySource> {
        match self.data {
            BodyDeclarationData::Binding(data) => Some(data.source),
            BodyDeclarationData::Item(data) => Some(data.name_source),
            BodyDeclarationData::ValueItem(data) => Some(data.name_source),
            BodyDeclarationData::Impl(_) => None,
            BodyDeclarationData::Field(data) => Some(BodySource {
                file_id: data.item.source.file_id,
                span: data.field.span,
            }),
            BodyDeclarationData::EnumVariant(data) => Some(BodySource {
                file_id: data.item.source.file_id,
                span: data.variant.name_span,
            }),
            BodyDeclarationData::Function(data) => Some(data.name_source),
        }
    }

    pub fn name(self) -> Option<&'a Name> {
        match self.data {
            BodyDeclarationData::Binding(data) => data.name.as_ref(),
            BodyDeclarationData::Item(data) => Some(&data.name),
            BodyDeclarationData::ValueItem(data) => Some(&data.name),
            BodyDeclarationData::Impl(_) => None,
            BodyDeclarationData::Field(data) => match data.field.key.as_ref() {
                Some(rg_item_tree::FieldKey::Named(name)) => Some(name),
                Some(rg_item_tree::FieldKey::Tuple(_)) | None => None,
            },
            BodyDeclarationData::EnumVariant(data) => Some(&data.variant.name),
            BodyDeclarationData::Function(data) => Some(&data.name),
        }
    }

    pub fn item_data(self) -> Option<&'a BodyItemData> {
        match self.data {
            BodyDeclarationData::Item(data) => Some(data),
            BodyDeclarationData::Binding(_)
            | BodyDeclarationData::ValueItem(_)
            | BodyDeclarationData::Impl(_)
            | BodyDeclarationData::Field(_)
            | BodyDeclarationData::EnumVariant(_)
            | BodyDeclarationData::Function(_) => None,
        }
    }

    pub fn value_item_data(self) -> Option<&'a BodyValueItemData> {
        match self.data {
            BodyDeclarationData::ValueItem(data) => Some(data),
            BodyDeclarationData::Binding(_)
            | BodyDeclarationData::Item(_)
            | BodyDeclarationData::Impl(_)
            | BodyDeclarationData::Field(_)
            | BodyDeclarationData::EnumVariant(_)
            | BodyDeclarationData::Function(_) => None,
        }
    }

    fn impl_data(self) -> Option<&'a BodyImplData> {
        match self.data {
            BodyDeclarationData::Impl(data) => Some(data),
            BodyDeclarationData::Binding(_)
            | BodyDeclarationData::Item(_)
            | BodyDeclarationData::ValueItem(_)
            | BodyDeclarationData::Field(_)
            | BodyDeclarationData::EnumVariant(_)
            | BodyDeclarationData::Function(_) => None,
        }
    }

    pub fn field_data(self) -> Option<BodyFieldData<'a>> {
        match self.data {
            BodyDeclarationData::Field(data) => Some(data),
            BodyDeclarationData::Binding(_)
            | BodyDeclarationData::Item(_)
            | BodyDeclarationData::ValueItem(_)
            | BodyDeclarationData::Impl(_)
            | BodyDeclarationData::EnumVariant(_)
            | BodyDeclarationData::Function(_) => None,
        }
    }

    fn function_data(self) -> Option<&'a BodyFunctionData> {
        match self.data {
            BodyDeclarationData::Function(data) => Some(data),
            BodyDeclarationData::Binding(_)
            | BodyDeclarationData::Item(_)
            | BodyDeclarationData::ValueItem(_)
            | BodyDeclarationData::Impl(_)
            | BodyDeclarationData::Field(_)
            | BodyDeclarationData::EnumVariant(_) => None,
        }
    }

    pub fn binding_data(self) -> Option<&'a BindingData> {
        match self.data {
            BodyDeclarationData::Binding(data) => Some(data),
            BodyDeclarationData::Item(_)
            | BodyDeclarationData::ValueItem(_)
            | BodyDeclarationData::Impl(_)
            | BodyDeclarationData::Field(_)
            | BodyDeclarationData::EnumVariant(_)
            | BodyDeclarationData::Function(_) => None,
        }
    }

    pub fn item_kind(self) -> Option<BodyItemKind> {
        self.item_data().map(|data| data.kind)
    }

    pub fn value_item_kind(self) -> Option<BodyValueItemKind> {
        self.value_item_data().map(|data| data.kind)
    }

    pub fn function_owner(self) -> Option<BodyFunctionOwner> {
        self.function_data().map(|data| data.owner)
    }

    pub fn impl_header(self) -> Option<(&'a TypeRef, Option<&'a TypeRef>)> {
        self.impl_data()
            .map(|data| (&data.self_ty, data.trait_ref.as_ref()))
    }
}
