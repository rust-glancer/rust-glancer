//! Source definitions retained by one DefMap before semantic item lowering.
//!
//! A local definition is a name-resolution identity, not necessarily a one-to-one semantic item.
//! Procedural macros make that boundary visible:
//!
//! ```text
//! #[proc_macro_derive(Stored)]
//! pub fn stored(input: TokenStream) -> TokenStream { /* ... */ }
//! ```
//!
//! The function `stored` remains the value-namespace implementation inside its defining crate,
//! while `Stored` is a separate macro-namespace export. The two definitions share source syntax
//! and are linked for navigation, but only the function is lowered as a semantic function. Keeping
//! those identities separate prevents macro lookup from exposing a function call signature.

use rg_ir_model::{
    DefMapRef, FileId, LocalDefId, LocalEnumVariantId, LocalEnumVariantRef, ModuleId, Span,
};
use rg_item_tree::{ItemKind, ItemTag, UserFacingAttrs, VisibilityLevel};
use rg_std::{MemorySize, Shrink};
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    ItemSource,
    binding::{NamespaceSet, Visibility},
};

/// One module-scope definition collected from source.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct LocalDefData {
    pub module: ModuleId,
    pub name: Name,
    pub kind: LocalDefKind,
    /// Namespace slots occupied by this declaration.
    ///
    /// Most declarations occupy one slot. Unit and tuple structs also contribute their constructor
    /// to the value namespace, so imports and shadowing must retain the set independently of kind.
    pub namespaces: NamespaceSet,
    pub visibility: VisibilityLevel,
    pub source: ItemSource,
    pub file_id: FileId,
    pub name_span: Option<Span>,
    pub span: Span,
    pub user_facing_attrs: UserFacingAttrs,
}

/// One enum variant that can be named through qualified paths and imports.
///
/// Variants stay semantically owned by `EnumData`, but def-map import resolution runs earlier than
/// semantic lowering. This record gives the name resolver a small, source-shaped handle that can
/// later be projected to `EnumVariantRef`.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct LocalEnumVariantData {
    pub module: ModuleId,
    pub enum_def: LocalDefId,
    pub name: Name,
    pub index: usize,
    /// Namespace slots occupied by this variant's source shape.
    ///
    /// Record variants are record-constructor paths in the type namespace. Tuple and unit variants
    /// additionally behave as values, so imports must retain both slots for them.
    pub namespaces: NamespaceSet,
    pub visibility: Visibility,
    pub file_id: FileId,
    pub name_span: Span,
    pub span: Span,
    pub user_facing_attrs: UserFacingAttrs,
}

/// Borrowed enum variant table entry paired with its stable def-map ref.
#[derive(Debug, Clone, Copy)]
pub struct LocalEnumVariantEntry<'a> {
    pub variant_ref: LocalEnumVariantRef,
    pub data: &'a LocalEnumVariantData,
}

impl<'a> LocalEnumVariantEntry<'a> {
    pub fn new(
        origin: DefMapRef,
        local_enum_variant: LocalEnumVariantId,
        data: &'a LocalEnumVariantData,
    ) -> Self {
        Self {
            variant_ref: LocalEnumVariantRef {
                origin,
                local_enum_variant,
            },
            data,
        }
    }
}

/// One module-owned impl block collected from source.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct LocalImplData {
    pub module: ModuleId,
    pub source: ItemSource,
    pub file_id: FileId,
    pub span: Span,
}

/// Module-scope definition kind that participates in def-map namespaces.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    SchemaRead,
    SchemaWrite,
    MemorySize,
    Shrink,
)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum LocalDefKind {
    #[display("const")]
    Const,
    #[display("enum")]
    Enum,
    #[display("fn")]
    Function,
    #[display("macro_definition")]
    MacroDefinition,
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
}

impl LocalDefKind {
    pub fn from_item_tag(tag: ItemTag) -> Option<Self> {
        match tag {
            ItemTag::Const => Some(Self::Const),
            ItemTag::Enum => Some(Self::Enum),
            ItemTag::Function => Some(Self::Function),
            ItemTag::MacroDefinition => Some(Self::MacroDefinition),
            ItemTag::Static => Some(Self::Static),
            ItemTag::Struct => Some(Self::Struct),
            ItemTag::Trait => Some(Self::Trait),
            ItemTag::TypeAlias => Some(Self::TypeAlias),
            ItemTag::Union => Some(Self::Union),
            ItemTag::AsmExpr
            | ItemTag::ExternBlock
            | ItemTag::ExternCrate
            | ItemTag::Impl
            | ItemTag::MacroCall
            | ItemTag::Module
            | ItemTag::Use => None,
        }
    }

    /// Return every scope slot contributed by the source declaration.
    ///
    /// `struct Unit;` and `struct Tuple(u8);` contribute both a type and a value constructor.
    /// `struct Record { value: u8 }` contributes only its type identity.
    pub fn scope_namespaces(self, item: &ItemKind) -> NamespaceSet {
        match self {
            Self::Const | Self::Function | Self::Static => NamespaceSet::VALUES,
            Self::Struct => {
                let ItemKind::Struct(struct_item) = item else {
                    return NamespaceSet::TYPES;
                };
                NamespaceSet::for_field_list(&struct_item.fields)
            }
            Self::Enum | Self::Trait | Self::TypeAlias | Self::Union => NamespaceSet::TYPES,
            Self::MacroDefinition => NamespaceSet::MACROS,
        }
    }
}
