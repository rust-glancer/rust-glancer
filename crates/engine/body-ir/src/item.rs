use rg_item_tree::{
    Documentation, FieldItem, FieldKey, FieldList, FunctionItem, GenericParams, ParamKind, TypeRef,
};
use rg_text::Name;

use crate::{
    body::BodySource,
    ids::{BodyFunctionId, BodyImplId, BodyItemRef, ScopeId},
};

/// One item declared inside a function body.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct BodyItemData {
    pub source: BodySource,
    pub name_source: BodySource,
    pub scope: ScopeId,
    pub kind: BodyItemKind,
    pub name: Name,
    pub docs: Option<Documentation>,
    pub generics: GenericParams,
    pub fields: FieldList,
}

impl BodyItemData {
    pub fn field(&self, index: usize) -> Option<&FieldItem> {
        self.fields.fields().get(index)
    }

    pub(crate) fn field_index(&self, key: &FieldKey) -> Option<usize> {
        self.fields
            .fields()
            .iter()
            .position(|field| field.key.as_ref() == Some(key))
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        if let Some(docs) = &mut self.docs {
            docs.shrink_to_fit();
        }
        self.generics.shrink_to_fit();
        self.fields.shrink_to_fit();
    }
}

/// Resolved access to one field declared on a body-local item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyFieldData<'a> {
    pub item: &'a BodyItemData,
    pub field: &'a FieldItem,
}

/// One impl block declared inside a function body.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct BodyImplData {
    pub source: BodySource,
    pub scope: ScopeId,
    pub generics: GenericParams,
    pub trait_ref: Option<TypeRef>,
    pub self_ty: TypeRef,
    pub self_item: Option<BodyItemRef>,
    pub functions: Vec<BodyFunctionId>,
}

impl BodyImplData {
    pub(crate) fn shrink_to_fit(&mut self) {
        self.generics.shrink_to_fit();
        if let Some(trait_ref) = &mut self.trait_ref {
            trait_ref.shrink_to_fit();
        }
        self.self_ty.shrink_to_fit();
        self.functions.shrink_to_fit();
    }
}

/// One function-like declaration inside a function body.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct BodyFunctionData {
    pub source: BodySource,
    pub name_source: BodySource,
    pub owner: BodyFunctionOwner,
    pub name: Name,
    pub docs: Option<Documentation>,
    pub declaration: FunctionItem,
}

impl BodyFunctionData {
    pub fn has_self_receiver(&self) -> bool {
        self.declaration
            .params
            .first()
            .is_some_and(|param| matches!(param.kind, ParamKind::SelfParam))
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        if let Some(docs) = &mut self.docs {
            docs.shrink_to_fit();
        }
        self.declaration.shrink_to_fit();
    }
}

/// Owner of a body-local function-like declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum BodyFunctionOwner {
    LocalImpl(BodyImplId),
}

/// Body-local item category.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub enum BodyItemKind {
    #[display("struct")]
    Struct,
}
