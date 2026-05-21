use rg_item_tree::{
    ConstItem, Documentation, EnumItem, EnumVariantItem, FieldItem, FieldKey, FieldList,
    FunctionItem, GenericParams, ParamKind, StaticItem, StructItem, TraitItem, TypeAliasItem,
    TypeRef, UnionItem,
};
use rg_text::Name;

use super::{
    body::BodySource,
    ids::{BodyFunctionId, BodyImplId, BodyItemId, BodyItemRef, BodyValueItemId, ScopeId},
};

/// One item declared inside a function body.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct BodyItemData {
    pub source: BodySource,
    pub name_source: BodySource,
    pub scope: ScopeId,
    pub owner: BodyItemOwner,
    pub kind: BodyItemKind,
    pub name: Name,
    pub docs: Option<Documentation>,
    pub declaration: BodyItemDeclaration,
}

impl BodyItemData {
    pub fn field(&self, index: usize) -> Option<&FieldItem> {
        self.fields().get(index)
    }

    pub fn fields(&self) -> &[FieldItem] {
        self.declaration.fields()
    }

    pub fn enum_variant(&self, index: usize) -> Option<&EnumVariantItem> {
        self.declaration.enum_variants().get(index)
    }

    pub fn enum_variants(&self) -> &[EnumVariantItem] {
        self.declaration.enum_variants()
    }

    pub fn generic_params(&self) -> Option<&GenericParams> {
        self.declaration.generic_params()
    }

    pub(crate) fn field_index(&self, key: &FieldKey) -> Option<usize> {
        self.fields()
            .iter()
            .position(|field| field.key.as_ref() == Some(key))
    }

    pub(crate) fn enum_variant_index(&self, name: &str) -> Option<usize> {
        self.declaration
            .enum_variants()
            .iter()
            .position(|variant| variant.name == name)
    }

    pub(crate) fn aliased_ty(&self) -> Option<&TypeRef> {
        self.declaration.aliased_ty()
    }

    pub(crate) fn is_nominal_type(&self) -> bool {
        matches!(
            self.kind,
            BodyItemKind::Struct | BodyItemKind::Enum | BodyItemKind::Union
        )
    }

    /// True for local type items that also introduce a bare value constructor.
    pub fn has_value_constructor(&self) -> bool {
        matches!(
            &self.declaration,
            BodyItemDeclaration::Struct(item)
                if matches!(item.fields, FieldList::Tuple(_) | FieldList::Unit)
        )
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        if let Some(docs) = &mut self.docs {
            docs.shrink_to_fit();
        }
        self.declaration.shrink_to_fit();
    }
}

/// Owner of a body-local type-namespace item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum BodyItemOwner {
    /// Lexical item declared directly in a body block, e.g. `fn f() { struct User; }`.
    LocalScope(ScopeId),
    /// Associated type item declared inside a body-local impl, e.g. `impl User { type Id = u32; }`.
    LocalImpl(BodyImplId),
}

/// Syntax-level declaration data for a body-local type-namespace item.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum BodyItemDeclaration {
    Struct(StructItem),
    Enum(EnumItem),
    Union(UnionItem),
    TypeAlias(TypeAliasItem),
    Trait(TraitItem),
}

impl BodyItemDeclaration {
    fn generic_params(&self) -> Option<&GenericParams> {
        match self {
            Self::Struct(item) => Some(&item.generics),
            Self::Enum(item) => Some(&item.generics),
            Self::Union(item) => Some(&item.generics),
            Self::TypeAlias(item) => Some(&item.generics),
            Self::Trait(item) => Some(&item.generics),
        }
    }

    fn fields(&self) -> &[FieldItem] {
        match self {
            Self::Struct(item) => item.fields.fields(),
            Self::Union(item) => &item.fields,
            Self::Enum(_) | Self::TypeAlias(_) | Self::Trait(_) => &[],
        }
    }

    fn enum_variants(&self) -> &[EnumVariantItem] {
        match self {
            Self::Enum(item) => &item.variants,
            Self::Struct(_) | Self::Union(_) | Self::TypeAlias(_) | Self::Trait(_) => &[],
        }
    }

    fn aliased_ty(&self) -> Option<&TypeRef> {
        match self {
            Self::TypeAlias(item) => item.aliased_ty.as_ref(),
            Self::Struct(_) | Self::Enum(_) | Self::Union(_) | Self::Trait(_) => None,
        }
    }

    fn shrink_to_fit(&mut self) {
        match self {
            Self::Struct(item) => item.shrink_to_fit(),
            Self::Enum(item) => item.shrink_to_fit(),
            Self::Union(item) => item.shrink_to_fit(),
            Self::TypeAlias(item) => item.shrink_to_fit(),
            Self::Trait(item) => item.shrink_to_fit(),
        }
    }
}

/// Resolved access to one field declared on a body-local item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyFieldData<'a> {
    pub item: &'a BodyItemData,
    pub field: &'a FieldItem,
}

/// Resolved access to one variant declared on a body-local enum item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyEnumVariantData<'a> {
    pub item: &'a BodyItemData,
    pub variant: &'a EnumVariantItem,
}

/// One value-namespace item declared inside a function body or body-local impl.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct BodyValueItemData {
    pub source: BodySource,
    pub name_source: BodySource,
    pub scope: ScopeId,
    pub owner: BodyValueItemOwner,
    pub kind: BodyValueItemKind,
    pub name: Name,
    pub docs: Option<Documentation>,
    pub declaration: BodyValueItemDeclaration,
}

impl BodyValueItemData {
    pub fn ty(&self) -> Option<&TypeRef> {
        match &self.declaration {
            BodyValueItemDeclaration::Const(item) => item.ty.as_ref(),
            BodyValueItemDeclaration::Static(item) => item.ty.as_ref(),
        }
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        if let Some(docs) = &mut self.docs {
            docs.shrink_to_fit();
        }
        self.declaration.shrink_to_fit();
    }
}

/// Owner of a body-local value item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum BodyValueItemOwner {
    LocalScope(ScopeId),
    LocalImpl(BodyImplId),
}

/// Body-local value item category.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    wincode::SchemaRead,
    wincode::SchemaWrite,
)]
pub enum BodyValueItemKind {
    #[display("const")]
    Const,
    #[display("static")]
    Static,
}

/// Syntax-level declaration data for a body-local value item.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum BodyValueItemDeclaration {
    Const(ConstItem),
    Static(StaticItem),
}

impl BodyValueItemDeclaration {
    fn shrink_to_fit(&mut self) {
        match self {
            Self::Const(item) => item.shrink_to_fit(),
            Self::Static(item) => item.shrink_to_fit(),
        }
    }
}

/// One impl block declared inside a function body.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct BodyImplData {
    pub source: BodySource,
    pub scope: ScopeId,
    pub generics: GenericParams,
    pub trait_ref: Option<TypeRef>,
    pub self_ty: TypeRef,
    pub self_item: Option<BodyItemRef>,
    pub functions: Vec<BodyFunctionId>,
    pub consts: Vec<BodyValueItemId>,
    pub types: Vec<BodyItemId>,
}

impl BodyImplData {
    pub(crate) fn shrink_to_fit(&mut self) {
        self.generics.shrink_to_fit();
        if let Some(trait_ref) = &mut self.trait_ref {
            trait_ref.shrink_to_fit();
        }
        self.self_ty.shrink_to_fit();
        self.functions.shrink_to_fit();
        self.consts.shrink_to_fit();
        self.types.shrink_to_fit();
    }
}

/// One function-like declaration inside a function body.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum BodyFunctionOwner {
    LocalScope(ScopeId),
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
    wincode::SchemaRead,
    wincode::SchemaWrite,
)]
pub enum BodyItemKind {
    #[display("struct")]
    Struct,
    #[display("enum")]
    Enum,
    #[display("union")]
    Union,
    #[display("type")]
    TypeAlias,
    #[display("trait")]
    Trait,
}
