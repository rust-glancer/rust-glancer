use rg_memsize::MemorySize;
use wincode::{SchemaRead, SchemaWrite};

use crate::ModuleRef;
use crate::declare_id;
use crate::ids::def_map::DefMapRef;

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
    pub origin: DefMapRef,
    pub id: TypeDefId,
}

impl TypeDefRef {
    pub fn new_struct(origin: DefMapRef, id: StructId) -> Self {
        Self {
            origin,
            id: TypeDefId::Struct(id),
        }
    }

    pub fn new_union(origin: DefMapRef, id: UnionId) -> Self {
        Self {
            origin,
            id: TypeDefId::Union(id),
        }
    }

    pub fn new_enum(origin: DefMapRef, id: EnumId) -> Self {
        Self {
            origin,
            id: TypeDefId::Enum(id),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct TraitRef {
    pub origin: DefMapRef,
    pub id: TraitId,
}

impl TraitRef {
    pub fn new(origin: DefMapRef, id: TraitId) -> Self {
        Self { origin, id }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct ImplRef {
    pub origin: DefMapRef,
    pub id: ImplId,
}

impl ImplRef {
    pub fn new(origin: DefMapRef, id: ImplId) -> Self {
        Self { origin, id }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct FunctionRef {
    pub origin: DefMapRef,
    pub id: FunctionId,
}

impl FunctionRef {
    pub fn new(origin: DefMapRef, id: FunctionId) -> Self {
        Self { origin, id }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct TypeAliasRef {
    pub origin: DefMapRef,
    pub id: TypeAliasId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct ConstRef {
    pub origin: DefMapRef,
    pub id: ConstId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
pub struct StaticRef {
    pub origin: DefMapRef,
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
    pub fn origin(self) -> DefMapRef {
        match self {
            Self::TypeDef(item) => item.origin,
            Self::Trait(item) => item.origin,
            Self::Impl(item) => item.origin,
            Self::Function(item) => item.origin,
            Self::TypeAlias(item) => item.origin,
            Self::Const(item) => item.origin,
            Self::Static(item) => item.origin,
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
    pub origin: DefMapRef,
    pub enum_id: EnumId,
    pub index: usize,
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
    pub fn semantic_ref(self, origin: DefMapRef) -> SemanticItemRef {
        match self {
            Self::Struct(id) => TypeDefRef {
                origin,
                id: TypeDefId::Struct(id),
            }
            .into(),
            Self::Union(id) => TypeDefRef {
                origin,
                id: TypeDefId::Union(id),
            }
            .into(),
            Self::Enum(id) => TypeDefRef {
                origin,
                id: TypeDefId::Enum(id),
            }
            .into(),
            Self::Trait(id) => TraitRef { origin, id }.into(),
            Self::Function(id) => FunctionRef { origin, id }.into(),
            Self::TypeAlias(id) => TypeAliasRef { origin, id }.into(),
            Self::Const(id) => ConstRef { origin, id }.into(),
            Self::Static(id) => StaticRef { origin, id }.into(),
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
