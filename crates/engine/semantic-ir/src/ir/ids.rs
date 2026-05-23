use rg_def_map::{ModuleRef, TargetRef};

macro_rules! impl_arena_id {
    ($($id:ty),+ $(,)?) => {
        $(
            impl rg_arena::ArenaId for $id {
                fn from_index(index: usize) -> Self {
                    Self(index)
                }

                fn index(self) -> usize {
                    self.0
                }
            }
        )+
    };
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub struct StructId(pub usize);

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub struct UnionId(pub usize);

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub struct EnumId(pub usize);

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub struct TraitId(pub usize);

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub struct ImplId(pub usize);

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub struct FunctionId(pub usize);

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub struct TypeAliasId(pub usize);

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub struct ConstId(pub usize);

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub struct StaticId(pub usize);

impl_arena_id!(
    StructId,
    UnionId,
    EnumId,
    TraitId,
    ImplId,
    FunctionId,
    TypeAliasId,
    ConstId,
    StaticId,
);

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub enum TypeDefId {
    Struct(StructId),
    Enum(EnumId),
    Union(UnionId),
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct TypeDefRef {
    pub target: TargetRef,
    pub id: TypeDefId,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct TraitRef {
    pub target: TargetRef,
    pub id: TraitId,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct ImplRef {
    pub target: TargetRef,
    pub id: ImplId,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct FunctionRef {
    pub target: TargetRef,
    pub id: FunctionId,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct TypeAliasRef {
    pub target: TargetRef,
    pub id: TypeAliasId,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct ConstRef {
    pub target: TargetRef,
    pub id: ConstId,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct StaticRef {
    pub target: TargetRef,
    pub id: StaticId,
}

/// Semantic item family used by read APIs that work with item-shaped facts.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
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
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    derive_more::From,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
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

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct FieldRef {
    pub owner: TypeDefRef,
    pub index: usize,
}

/// Stable identity for one enum variant.
///
/// Variants are currently stored as children of `EnumData` rather than promoted to top-level
/// semantic items. The explicit ref gives analysis enough identity for navigation and type queries
/// without changing that storage model prematurely.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct EnumVariantRef {
    pub target: TargetRef,
    pub enum_id: EnumId,
    pub index: usize,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct TraitImplRef {
    pub impl_ref: ImplRef,
    pub trait_ref: TraitRef,
}

/// Best-effort answer for "does this trait impl apply to this receiver type?".
///
/// `Maybe` is a first-class result because this project intentionally prefers useful trait-method
/// candidates over trying to prove generic bounds and where-clauses precisely.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
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

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
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

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub enum AssocItemId {
    Function(FunctionId),
    TypeAlias(TypeAliasId),
    Const(ConstId),
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub enum ItemOwner {
    Module(ModuleRef),
    Trait(TraitId),
    Impl(ImplId),
}
