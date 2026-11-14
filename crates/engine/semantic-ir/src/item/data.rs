use rg_def_map::ItemSource;
use rg_ir_model::{
    ItemOwner, LocalDefRef, LocalImplRef, ModuleRef, Mutability, SourceId, TraitDefRef, TypeDefRef,
};
use rg_item_tree::{
    Documentation, EnumVariantItem, FieldItem, FieldList, GenericParams, ImplPolarity, ParamKind,
    TypeBound, TypeRef, VisibilityLevel,
};
use rg_std::{ExpectedUnique, MemorySize, Shrink};
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

use super::signature::{ConstSignature, FunctionSignature, TypeAliasSignature};

/// Borrowed view over one field plus the semantic owner facts needed by analysis.
#[derive(Debug, Clone, Copy)]
pub struct FieldData<'a> {
    pub owner_module: ModuleRef,
    pub source: ItemSource,
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
    pub source: ItemSource,
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

/// Trait header: the trait's name, generic parameters and supertraits.
///
/// Associated functions, types and constants live in the trait's [`crate::AssociatedItemStore`].
/// Keeping them separate lets callers inspect the trait's bounds before asking for its members.
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
    pub is_unsafe: bool,
    pub is_auto: bool,
}

/// Information from the `impl<T> Trait for Type<T>` part of a block.
///
/// Lookup matches this header against a receiver type. The header also supplies `Self` and the
/// generic parameters and bounds inherited by the block's members. The functions, types and
/// constants between the braces live in a separate [`crate::AssociatedItemStore`], selected by
/// [`rg_ir_model::ImplRef::member_store`].
///
/// `resolved_*` fields are lossy: invalid or ambiguous headers stay visible until
/// query code decides whether it needs a single known definition.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ImplData {
    pub local_impl: LocalImplRef,
    pub source: ItemSource,
    pub owner: ModuleRef,
    pub generics: GenericParams,
    pub trait_ref: Option<TypeRef>,
    pub self_ty: TypeRef,
    pub resolved_self_ty: ExpectedUnique<TypeDefRef>,
    pub resolved_trait_ref: ExpectedUnique<TraitDefRef>,
    pub is_unsafe: bool,
    pub polarity: ImplPolarity,
}

/// Function signature and source identity.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct FunctionData {
    pub local_def: Option<LocalDefRef>,
    pub source: ItemSource,
    pub location: SourceId,
    pub name_location: Option<SourceId>,
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
            .is_some_and(|param| matches!(param.kind, ParamKind::SelfParam(_)))
    }
}

/// Type alias signature and optional aliased type.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct TypeAliasData {
    pub local_def: Option<LocalDefRef>,
    pub source: ItemSource,
    pub location: SourceId,
    pub name_location: Option<SourceId>,
    pub owner: ItemOwner,
    pub name: Name,
    pub visibility: VisibilityLevel,
    pub docs: Option<Documentation>,
    pub signature: TypeAliasSignature,
}

/// Const identity and signature. Evaluating its initializer is a separate body query.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ConstData {
    pub local_def: Option<LocalDefRef>,
    pub source: ItemSource,
    pub location: SourceId,
    pub name_location: Option<SourceId>,
    pub owner: ItemOwner,
    pub name: Name,
    pub visibility: VisibilityLevel,
    pub docs: Option<Documentation>,
    pub signature: ConstSignature,
    /// The source table retains a bounded preview at this occurrence, independently of meaning.
    pub initializer_location: Option<SourceId>,
}

/// Static identity and declared type. Its initial value belongs to body facts.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct StaticData {
    pub local_def: LocalDefRef,
    pub source: ItemSource,
    pub location: SourceId,
    pub name_location: Option<SourceId>,
    pub owner: ModuleRef,
    pub name: Name,
    pub visibility: VisibilityLevel,
    pub docs: Option<Documentation>,
    pub ty: Option<TypeRef>,
    pub mutability: Mutability,
    /// The source table retains a bounded preview at this occurrence, independently of meaning.
    pub initializer_location: Option<SourceId>,
}
