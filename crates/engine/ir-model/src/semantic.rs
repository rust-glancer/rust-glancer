use rg_memsize::MemorySize;
use wincode::{SchemaRead, SchemaWrite};

use crate::declare_id;
use crate::{ModuleRef, TargetRef};

declare_id! {
    pub struct StructId;
    pub struct UnionId;
    pub struct EnumId;
    pub struct TraitId;
    pub struct ImplId;
    pub struct FunctionId;
    pub struct TypeAliasId;
    pub struct ConstId;
    pub struct StaticId;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub enum TypeDefId {
    Struct(StructId),
    Enum(EnumId),
    Union(UnionId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct TypeDefRef {
    pub target: TargetRef,
    pub id: TypeDefId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct TraitRef {
    pub target: TargetRef,
    pub id: TraitId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct ImplRef {
    pub target: TargetRef,
    pub id: ImplId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct FunctionRef {
    pub target: TargetRef,
    pub id: FunctionId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct TypeAliasRef {
    pub target: TargetRef,
    pub id: TypeAliasId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct ConstRef {
    pub target: TargetRef,
    pub id: ConstId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct StaticRef {
    pub target: TargetRef,
    pub id: StaticId,
}

/// Semantic item family used by read APIs that work with item-shaped facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub enum SemanticItemKind {
    Struct,
    Enum,
    Union,
    Trait,
    Impl,
    Function,
    TypeAlias,
    Const,
    Static,
}

/// Stable identity for one top-level or associated semantic item.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, derive_more::From, SchemaRead, SchemaWrite, MemorySize,
)]
pub enum SemanticItemRef {
    TypeDef(TypeDefRef),
    Trait(TraitRef),
    Impl(ImplRef),
    Function(FunctionRef),
    TypeAlias(TypeAliasRef),
    Const(ConstRef),
    Static(StaticRef),
}

impl SemanticItemRef {
    pub fn target(self) -> TargetRef {
        match self {
            Self::TypeDef(item) => item.target,
            Self::Trait(item) => item.target,
            Self::Impl(item) => item.target,
            Self::Function(item) => item.target,
            Self::TypeAlias(item) => item.target,
            Self::Const(item) => item.target,
            Self::Static(item) => item.target,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct FieldRef {
    pub owner: TypeDefRef,
    pub index: usize,
}

/// Stable identity for one enum variant.
///
/// Variants are stored as children of `EnumData` rather than promoted to top-level semantic items.
/// The explicit ref gives higher layers enough identity for navigation and type queries without
/// dictating that storage model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct EnumVariantRef {
    pub target: TargetRef,
    pub enum_id: EnumId,
    pub index: usize,
}

/// Stable identity for any declaration contributed by Semantic IR.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, derive_more::From, SchemaRead, SchemaWrite, MemorySize,
)]
pub enum SemanticDeclarationRef {
    #[from(
        SemanticItemRef,
        TypeDefRef,
        TraitRef,
        ImplRef,
        FunctionRef,
        TypeAliasRef,
        ConstRef,
        StaticRef
    )]
    Item(SemanticItemRef),
    #[from]
    Field(FieldRef),
    #[from]
    EnumVariant(EnumVariantRef),
}

impl SemanticDeclarationRef {
    pub fn target(self) -> TargetRef {
        match self {
            Self::Item(item) => item.target(),
            Self::Field(field) => field.owner.target,
            Self::EnumVariant(variant) => variant.target,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct TraitImplRef {
    pub impl_ref: ImplRef,
    pub trait_ref: TraitRef,
}

/// Best-effort answer for "does this trait impl apply to this receiver type?".
///
/// `Maybe` is a first-class result because this project intentionally prefers useful trait-method
/// candidates over trying to prove generic bounds and where-clauses precisely.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, SchemaRead, SchemaWrite, MemorySize,
)]
#[memsize(leaf)]
pub enum TraitApplicability {
    Yes,
    Maybe,
    No,
}

impl TraitApplicability {
    pub fn is_applicable(self) -> bool {
        !matches!(self, Self::No)
    }

    pub fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::No, _) | (_, Self::No) => Self::No,
            (Self::Maybe, _) | (_, Self::Maybe) => Self::Maybe,
            (Self::Yes, Self::Yes) => Self::Yes,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub enum ItemId {
    Struct(StructId),
    Union(UnionId),
    Enum(EnumId),
    Trait(TraitId),
    Function(FunctionId),
    TypeAlias(TypeAliasId),
    Const(ConstId),
    Static(StaticId),
}

impl ItemId {
    pub fn semantic_ref(self, target: TargetRef) -> SemanticItemRef {
        match self {
            Self::Struct(id) => TypeDefRef {
                target,
                id: TypeDefId::Struct(id),
            }
            .into(),
            Self::Union(id) => TypeDefRef {
                target,
                id: TypeDefId::Union(id),
            }
            .into(),
            Self::Enum(id) => TypeDefRef {
                target,
                id: TypeDefId::Enum(id),
            }
            .into(),
            Self::Trait(id) => TraitRef { target, id }.into(),
            Self::Function(id) => FunctionRef { target, id }.into(),
            Self::TypeAlias(id) => TypeAliasRef { target, id }.into(),
            Self::Const(id) => ConstRef { target, id }.into(),
            Self::Static(id) => StaticRef { target, id }.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub enum AssocItemId {
    Function(FunctionId),
    TypeAlias(TypeAliasId),
    Const(ConstId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub enum ItemOwner {
    Module(ModuleRef),
    Trait(TraitId),
    Impl(ImplId),
}
