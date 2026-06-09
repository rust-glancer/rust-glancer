use crate::items::{
    Documentation, EnumVariantItem, FieldItem, FieldList, GenericParams, Mutability, ParamKind,
    TypeBound, TypeRef, VisibilityLevel,
};
use rg_parse::{FileId, Span};
use rg_std::{MemorySize, Shrink, UniqueVec};
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    AssocItemId, FunctionRef, ItemOwner, LocalDefRef, LocalImplRef, ModuleRef, TraitRef, TypeDefRef,
};

use super::{
    signature::{ConstSignature, FunctionSignature, TypeAliasSignature},
    source::ItemSource,
};

/// Borrowed view over one field plus the semantic owner facts needed by analysis.
#[derive(Debug, Clone, Copy)]
pub struct FieldData<'a> {
    pub owner_module: ModuleRef,
    pub file_id: FileId,
    pub field: &'a FieldItem,
}

/// Borrowed view over one enum variant plus the owning enum facts needed by analysis.
///
/// The owner data is repeated here so callers do not have to re-open the enum just to answer
/// editor questions such as "what type does this variant construct?".
#[derive(Debug, Clone, Copy)]
pub struct EnumVariantData<'a> {
    pub owner: TypeDefRef,
    pub owner_module: ModuleRef,
    pub file_id: FileId,
    pub variant: &'a EnumVariantItem,
}

/// Nominal struct lowered from a module item.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct StructData {
    pub local_def: LocalDefRef,
    pub source: ItemSource,
    pub owner: ModuleRef,
    pub name: Name,
    pub visibility: VisibilityLevel,
    pub docs: Option<Documentation>,
    pub generics: GenericParams,
    pub fields: FieldList,
}

/// Nominal union lowered from a module item.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct UnionData {
    pub local_def: LocalDefRef,
    pub source: ItemSource,
    pub owner: ModuleRef,
    pub name: Name,
    pub visibility: VisibilityLevel,
    pub docs: Option<Documentation>,
    pub generics: GenericParams,
    pub fields: Vec<FieldItem>,
}

/// Enum definition together with variant payloads.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct EnumData {
    pub local_def: LocalDefRef,
    pub source: ItemSource,
    pub owner: ModuleRef,
    pub name: Name,
    pub visibility: VisibilityLevel,
    pub docs: Option<Documentation>,
    pub generics: GenericParams,
    pub variants: Vec<EnumVariantItem>,
}

/// Trait signature and associated items.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct TraitData {
    pub local_def: LocalDefRef,
    pub source: ItemSource,
    pub owner: ModuleRef,
    pub name: Name,
    pub visibility: VisibilityLevel,
    pub docs: Option<Documentation>,
    pub generics: GenericParams,
    pub super_traits: Vec<TypeBound>,
    pub items: Vec<AssocItemId>,
    pub is_unsafe: bool,
}

impl TraitData {
    pub fn functions(&self) -> impl Iterator<Item = FunctionRef> {
        self.items.iter().filter_map(|item| {
            if let AssocItemId::Function(id) = item {
                Some(FunctionRef {
                    origin: self.local_def.origin,
                    id: *id,
                })
            } else {
                None
            }
        })
    }
}

/// Impl block header and associated items.
///
/// `resolved_*` fields are intentionally lossy and optimistic: they record all type/trait targets
/// that our current path resolver can identify, without attempting a real trait solver.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ImplData {
    pub local_impl: LocalImplRef,
    pub source: ItemSource,
    pub owner: ModuleRef,
    pub generics: GenericParams,
    pub trait_ref: Option<TypeRef>,
    pub self_ty: TypeRef,
    pub resolved_self_tys: UniqueVec<TypeDefRef>,
    pub resolved_trait_refs: UniqueVec<TraitRef>,
    pub items: Vec<AssocItemId>,
    pub is_unsafe: bool,
}

impl ImplData {
    pub fn functions(&self) -> impl Iterator<Item = FunctionRef> {
        self.items.iter().filter_map(|item| {
            if let AssocItemId::Function(id) = item {
                Some(FunctionRef {
                    origin: self.local_impl.origin,
                    id: *id,
                })
            } else {
                None
            }
        })
    }
}

/// Function signature and source identity.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct FunctionData {
    pub local_def: Option<LocalDefRef>,
    pub source: ItemSource,
    pub span: Span,
    pub name_span: Option<Span>,
    pub owner: ItemOwner,
    pub name: Name,
    pub visibility: VisibilityLevel,
    pub docs: Option<Documentation>,
    pub signature: FunctionSignature,
}

impl FunctionData {
    pub fn has_self_receiver(&self) -> bool {
        self.signature
            .params()
            .first()
            .is_some_and(|param| matches!(param.kind, ParamKind::SelfParam))
    }
}

/// Type alias signature and optional aliased type.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct TypeAliasData {
    pub local_def: Option<LocalDefRef>,
    pub source: ItemSource,
    pub span: Span,
    pub name_span: Option<Span>,
    pub owner: ItemOwner,
    pub name: Name,
    pub visibility: VisibilityLevel,
    pub docs: Option<Documentation>,
    pub signature: TypeAliasSignature,
}

/// Const signature and optional value body owner.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ConstData {
    pub local_def: Option<LocalDefRef>,
    pub source: ItemSource,
    pub span: Span,
    pub name_span: Option<Span>,
    pub owner: ItemOwner,
    pub name: Name,
    pub visibility: VisibilityLevel,
    pub docs: Option<Documentation>,
    pub signature: ConstSignature,
}

/// Module-level static item.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct StaticData {
    pub local_def: LocalDefRef,
    pub source: ItemSource,
    pub span: Span,
    pub name_span: Option<Span>,
    pub owner: ModuleRef,
    pub name: Name,
    pub visibility: VisibilityLevel,
    pub docs: Option<Documentation>,
    pub ty: Option<TypeRef>,
    pub mutability: Mutability,
}
