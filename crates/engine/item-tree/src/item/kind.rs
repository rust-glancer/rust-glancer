use super::{
    decl::{
        ConstItem, EnumItem, FunctionItem, ImplItem, StaticItem, StructItem, TraitItem,
        TypeAliasItem, UnionItem,
    },
    import::{ExternCrateItem, UseItem},
    module::ModuleItem,
};

/// Payload-bearing item kind.
///
/// Unit variants are enough for plain local definitions. Structured payloads live inline in the
/// file item arena so lowering avoids one heap allocation per payload-bearing item.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    derive_more::Display,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub enum ItemKind {
    #[display("asm")]
    AsmExpr,
    #[display("const")]
    Const(ConstItem),
    #[display("enum")]
    Enum(EnumItem),
    #[display("extern_block")]
    ExternBlock,
    #[display("extern_crate")]
    ExternCrate(ExternCrateItem),
    #[display("fn")]
    Function(FunctionItem),
    #[display("impl")]
    Impl(ImplItem),
    #[display("macro_definition")]
    MacroDefinition,
    #[display("module")]
    Module(ModuleItem),
    #[display("static")]
    Static(StaticItem),
    #[display("struct")]
    Struct(StructItem),
    #[display("trait")]
    Trait(TraitItem),
    #[display("type_alias")]
    TypeAlias(TypeAliasItem),
    #[display("union")]
    Union(UnionItem),
    #[display("use")]
    Use(UseItem),
}

impl ItemKind {
    /// Returns payload-independent item classification.
    pub fn tag(&self) -> ItemTag {
        match self {
            Self::AsmExpr => ItemTag::AsmExpr,
            Self::Const(_) => ItemTag::Const,
            Self::Enum(_) => ItemTag::Enum,
            Self::ExternBlock => ItemTag::ExternBlock,
            Self::ExternCrate(_) => ItemTag::ExternCrate,
            Self::Function(_) => ItemTag::Function,
            Self::Impl(_) => ItemTag::Impl,
            Self::MacroDefinition => ItemTag::MacroDefinition,
            Self::Module(_) => ItemTag::Module,
            Self::Static(_) => ItemTag::Static,
            Self::Struct(_) => ItemTag::Struct,
            Self::Trait(_) => ItemTag::Trait,
            Self::TypeAlias(_) => ItemTag::TypeAlias,
            Self::Union(_) => ItemTag::Union,
            Self::Use(_) => ItemTag::Use,
        }
    }
}

/// Payload-independent item classification.
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
pub enum ItemTag {
    #[display("asm")]
    AsmExpr,
    #[display("const")]
    Const,
    #[display("enum")]
    Enum,
    #[display("extern_block")]
    ExternBlock,
    #[display("extern_crate")]
    ExternCrate,
    #[display("fn")]
    Function,
    #[display("impl")]
    Impl,
    #[display("macro_definition")]
    MacroDefinition,
    #[display("module")]
    Module,
    #[display("static")]
    Static,
    #[display("struct")]
    Struct,
    #[display("trait")]
    Trait,
    #[display("type_alias")]
    TypeAlias,
    #[display("union")]
    Union,
    #[display("use")]
    Use,
}
