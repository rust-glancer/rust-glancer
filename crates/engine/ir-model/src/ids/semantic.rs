use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::{ItemStoreId, ItemStoreRef, ModuleRef, declare_id, ids::def_map::DefMapRef};

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

    /// Owner-local ordinal of one opaque `impl Trait` occurrence.
    ///
    /// For `fn make() -> (impl Debug, impl Debug)`, the two return components get different IDs
    /// even though their bounds are identical. [`OpaqueTyRef`] pairs this ordinal with the
    /// signature owner; the ordinal alone is not a project-wide identity.
    pub struct OpaqueTyId;

    /// Index of a lifetime within one owner's lifetime-parameter lane.
    ///
    /// Explicit lifetimes form their own leading declaration group and cannot interleave with the
    /// type/const parameters. The semantic `Generics` view later combines both lanes and inserts
    /// special parameters such as trait `Self` in semantic argument order.
    pub struct LocalLifetimeParamId;

    /// Index of a type or const parameter within one shared owner-local lane.
    ///
    /// The shared lane preserves source order in declarations such as
    /// `struct Buffer<T, const N: usize, U>`. Whether an index names a type or const is carried by
    /// [`TypeParamRef`] or [`ConstParamRef`], not by this index itself. Implicit type parameters,
    /// such as trait `Self` and argument-position `impl Trait`, also live in this lane.
    pub struct LocalTypeOrConstParamId;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub enum TypeDefId {
    Struct(StructId),
    Enum(EnumId),
    Union(UnionId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
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

/// Stable reference to a trait declaration, without `Self` or generic arguments applied.
///
/// Type-level code uses this only as the definition identity. An instantiated use such as
/// `Vec<User>: IntoIterator` is represented by a semantic `TraitApplication` instead.
/// The trait header belongs to `origin`'s scope store; [`Self::member_store`] selects the
/// separate store containing its associated declarations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct TraitDefRef {
    pub origin: DefMapRef,
    pub id: TraitId,
}

impl TraitDefRef {
    pub fn new(origin: DefMapRef, id: TraitId) -> Self {
        Self { origin, id }
    }

    pub fn member_store(self) -> ItemStoreRef {
        ItemStoreRef {
            origin: self.origin,
            id: ItemStoreId::Trait(self.id),
        }
    }
}

/// Identifies an impl block's header in the enclosing scope.
///
/// [`Self::member_store`] selects the declarations inside that block. Two impl blocks for the
/// same type have different header IDs and different member stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct ImplRef {
    pub origin: DefMapRef,
    pub id: ImplId,
}

impl ImplRef {
    pub fn new(origin: DefMapRef, id: ImplId) -> Self {
        Self { origin, id }
    }

    pub fn member_store(self) -> ItemStoreRef {
        ItemStoreRef {
            origin: self.origin,
            id: ItemStoreId::Impl(self.id),
        }
    }
}

/// An individual function declaration, either free or associated with a trait/impl.
///
/// Keep `store` with `id`: two methods can have the same local ID in different member stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct FunctionRef {
    pub store: ItemStoreRef,
    pub id: FunctionId,
}

impl FunctionRef {
    pub fn new(store: ItemStoreRef, id: FunctionId) -> Self {
        Self { store, id }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct TypeAliasRef {
    pub store: ItemStoreRef,
    pub id: TypeAliasId,
}

impl TypeAliasRef {
    pub fn new(store: ItemStoreRef, id: TypeAliasId) -> Self {
        Self { store, id }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct ConstRef {
    pub store: ItemStoreRef,
    pub id: ConstId,
}

impl ConstRef {
    pub fn new(store: ItemStoreRef, id: ConstId) -> Self {
        Self { store, id }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct StaticRef {
    pub origin: DefMapRef,
    pub id: StaticId,
}

impl StaticRef {
    pub fn new(origin: DefMapRef, id: StaticId) -> Self {
        Self { origin, id }
    }
}

/// A const or static declaration whose initializer can supply a stored compile-time value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, derive_more::From)]
pub enum ConstOrStaticRef {
    Const(ConstRef),
    Static(StaticRef),
}

impl ConstOrStaticRef {
    pub fn origin(self) -> DefMapRef {
        match self {
            Self::Const(item) => item.store.origin,
            Self::Static(item) => item.origin,
        }
    }
}

/// Definition whose signature can introduce or inherit generic parameters.
///
/// Consts and statics do not declare their own parameters in stable Rust, but associated items can
/// still inherit parameters from a trait or impl. Keeping every signature owner representable lets
/// callers ask one parent-generics question without inventing a second owner family.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    derive_more::From,
    SchemaRead,
    SchemaWrite,
    MemorySize,
    Shrink,
)]
#[shrink(leaf)]
pub enum GenericDefRef {
    TypeDef(TypeDefRef),
    Trait(TraitDefRef),
    Impl(ImplRef),
    Function(FunctionRef),
    TypeAlias(TypeAliasRef),
    Const(ConstRef),
    Static(StaticRef),
}

impl GenericDefRef {
    pub fn origin(self) -> DefMapRef {
        match self {
            Self::TypeDef(def) => def.origin,
            Self::Trait(def) => def.origin,
            Self::Impl(def) => def.origin,
            Self::Function(def) => def.store.origin,
            Self::TypeAlias(def) => def.store.origin,
            Self::Const(def) => def.store.origin,
            Self::Static(def) => def.origin,
        }
    }
}

impl From<SemanticItemRef> for GenericDefRef {
    fn from(item: SemanticItemRef) -> Self {
        match item {
            SemanticItemRef::TypeDef(def) => Self::TypeDef(def),
            SemanticItemRef::Trait(def) => Self::Trait(def),
            SemanticItemRef::Impl(def) => Self::Impl(def),
            SemanticItemRef::Function(def) => Self::Function(def),
            SemanticItemRef::TypeAlias(def) => Self::TypeAlias(def),
            SemanticItemRef::Const(def) => Self::Const(def),
            SemanticItemRef::Static(def) => Self::Static(def),
        }
    }
}

/// Owner-scoped identity of a lifetime parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct LifetimeParamRef {
    pub owner: GenericDefRef,
    pub local_id: LocalLifetimeParamId,
}

/// Owner-scoped identity of a type parameter.
///
/// Type and const parameters share one local index space so `struct S<T, const N: usize, U>` keeps
/// the declaration order needed by canonical generic arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct TypeParamRef {
    pub owner: GenericDefRef,
    pub local_id: LocalTypeOrConstParamId,
}

/// Owner-scoped identity of a const parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct ConstParamRef {
    pub owner: GenericDefRef,
    pub local_id: LocalTypeOrConstParamId,
}

/// Owner-local identity of one `impl Trait` occurrence.
///
/// Bounds are deliberately absent: two occurrences with identical bounds are still different
/// opaque types, while one occurrence keeps its identity if its predicates are queried elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct OpaqueTyRef {
    pub owner: GenericDefRef,
    pub id: OpaqueTyId,
}

/// Parameter identity used by semantic types and ordered argument lists.
///
/// The enum variant makes the parameter kind part of the identity. Names, bounds, defaults, and
/// provenance remain queryable declaration data and therefore cannot accidentally affect type
/// equality.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    derive_more::From,
    SchemaRead,
    SchemaWrite,
    MemorySize,
    Shrink,
)]
#[shrink(leaf)]
pub enum GenericParamRef {
    Lifetime(LifetimeParamRef),
    Type(TypeParamRef),
    Const(ConstParamRef),
}

impl GenericParamRef {
    pub fn owner(self) -> GenericDefRef {
        match self {
            Self::Lifetime(param) => param.owner,
            Self::Type(param) => param.owner,
            Self::Const(param) => param.owner,
        }
    }
}

/// Semantic item family used by read APIs that work with item-shaped facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
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
    SchemaRead,
    SchemaWrite,
    MemorySize,
    Shrink,
)]
#[shrink(leaf)]
pub enum SemanticItemRef {
    TypeDef(TypeDefRef),
    Trait(TraitDefRef),
    Impl(ImplRef),
    Function(FunctionRef),
    TypeAlias(TypeAliasRef),
    Const(ConstRef),
    Static(StaticRef),
}

impl SemanticItemRef {
    /// Keep this selector when reading item data; an origin alone does not identify a member arena.
    pub fn store(self) -> ItemStoreRef {
        match self {
            Self::Const(item) => item.store,
            Self::Function(item) => item.store,
            Self::TypeAlias(item) => item.store,
            _ => ItemStoreRef::scope(self.origin()),
        }
    }

    /// The lexical scope for resolution and source routing, shared with the enclosing trait/impl.
    pub fn origin(self) -> DefMapRef {
        match self {
            Self::TypeDef(item) => item.origin,
            Self::Trait(item) => item.origin,
            Self::Impl(item) => item.origin,
            Self::Function(item) => item.store.origin,
            Self::TypeAlias(item) => item.store.origin,
            Self::Const(item) => item.store.origin,
            Self::Static(item) => item.origin,
        }
    }
}

impl From<GenericDefRef> for SemanticItemRef {
    fn from(def: GenericDefRef) -> Self {
        match def {
            GenericDefRef::TypeDef(def) => Self::TypeDef(def),
            GenericDefRef::Trait(def) => Self::Trait(def),
            GenericDefRef::Impl(def) => Self::Impl(def),
            GenericDefRef::Function(def) => Self::Function(def),
            GenericDefRef::TypeAlias(def) => Self::TypeAlias(def),
            GenericDefRef::Const(def) => Self::Const(def),
            GenericDefRef::Static(def) => Self::Static(def),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct FieldRef {
    pub owner: TypeDefRef,
    pub index: usize,
}

/// Stable identity for one enum variant.
///
/// Variants are stored as children of `EnumData` rather than promoted to top-level semantic items.
/// The explicit ref gives higher layers enough identity for navigation and type queries without
/// dictating that storage model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct EnumVariantRef {
    pub origin: DefMapRef,
    pub enum_id: EnumId,
    pub index: usize,
}

/// Stable identity for one field declared by an enum variant.
///
/// Variant fields cannot use [`FieldRef`]: that identity is intentionally rooted at a nominal
/// type, while two variants of the same enum may both declare a field with the same name. Keeping
/// the variant in the identity preserves the declaration that completion and navigation mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct EnumVariantFieldRef {
    pub owner: EnumVariantRef,
    pub index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct TraitImplRef {
    pub impl_ref: ImplRef,
    pub trait_ref: TraitDefRef,
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
    SchemaRead,
    SchemaWrite,
    MemorySize,
    Shrink,
)]
#[memsize(leaf)]
#[shrink(leaf)]
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

    pub fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::Yes, _) | (_, Self::Yes) => Self::Yes,
            (Self::Maybe, _) | (_, Self::Maybe) => Self::Maybe,
            (Self::No, Self::No) => Self::No,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
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
            Self::Struct(id) => TypeDefRef::new_struct(origin, id).into(),
            Self::Union(id) => TypeDefRef::new_union(origin, id).into(),
            Self::Enum(id) => TypeDefRef::new_enum(origin, id).into(),
            Self::Trait(id) => TraitDefRef::new(origin, id).into(),
            Self::Function(id) => FunctionRef::new(ItemStoreRef::scope(origin), id).into(),
            Self::TypeAlias(id) => TypeAliasRef::new(ItemStoreRef::scope(origin), id).into(),
            Self::Const(id) => ConstRef::new(ItemStoreRef::scope(origin), id).into(),
            Self::Static(id) => StaticRef::new(origin, id).into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub enum AssocItemId {
    Function(FunctionId),
    TypeAlias(TypeAliasId),
    Const(ConstId),
}

impl AssocItemId {
    pub fn semantic_ref(self, store: ItemStoreRef) -> SemanticItemRef {
        match self {
            Self::Function(id) => FunctionRef::new(store, id).into(),
            Self::TypeAlias(id) => TypeAliasRef::new(store, id).into(),
            Self::Const(id) => ConstRef::new(store, id).into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub enum ItemOwner {
    Module(ModuleRef),
    Trait(TraitId),
    Impl(ImplId),
}

impl ItemOwner {
    /// Trait and impl IDs name headers in the enclosing scope, rather than entries in the
    /// member arena being selected here. Module-owned declarations use the scope arena itself.
    pub fn item_store(self, origin: DefMapRef) -> ItemStoreRef {
        match self {
            Self::Module(module) => ItemStoreRef::scope(module.origin),
            Self::Trait(id) => TraitDefRef { origin, id }.member_store(),
            Self::Impl(id) => ImplRef { origin, id }.member_store(),
        }
    }
}
