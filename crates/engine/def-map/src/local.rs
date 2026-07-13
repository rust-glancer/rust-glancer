use rg_ir_model::items::{
    BuiltinMacroKind, Documentation, ItemKind, ItemTag, MacroDefinitionItem, VisibilityLevel,
};
use rg_ir_model::{
    CrateRef, DefMapRef, LocalDefId, LocalDefRef, LocalEnumVariantId, LocalEnumVariantRef,
    ModuleId, hir::source::ItemSource,
};
use rg_macro_runtime::DeclarativeMacroDefinition;
use rg_parse::{FileId, Span};
use rg_std::{MemorySize, Shrink};
use rg_text::{Name, RustEdition};
use rg_tt::TopSubtree;
use wincode::{SchemaRead, SchemaWrite};

use crate::scope::{NamespaceSet, Visibility};

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

/// Macro definition facts retained after def-map freezing.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct MacroDefinitionData {
    pub edition: RustEdition,
    /// Crate that `$crate` inside this macro body should resolve to when expanded.
    pub dollar_crate: CrateRef,
    /// User-facing documentation attached to the macro definition item.
    pub docs: Option<Documentation>,
    /// Compiler hook that should run instead of declarative expansion, if any.
    pub builtin: Option<BuiltinMacroKind>,
    #[shrink(skip)]
    pub payload: MacroDefinitionPayload,
}

impl MacroDefinitionData {
    pub fn from_item(
        item: &MacroDefinitionItem,
        docs: Option<Documentation>,
        edition: RustEdition,
        dollar_crate: CrateRef,
    ) -> Self {
        Self {
            edition,
            dollar_crate,
            docs,
            builtin: Self::builtin_from_item(item),
            payload: MacroDefinitionPayload::from_item(item),
        }
    }

    fn builtin_from_item(item: &MacroDefinitionItem) -> Option<BuiltinMacroKind> {
        match item {
            MacroDefinitionItem::MacroRules { attrs, .. } => attrs.builtin,
            MacroDefinitionItem::MacroDef { .. } => None,
        }
    }
}

/// Token-tree payload needed to compile a collected declarative macro.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum MacroDefinitionPayload {
    MacroRules {
        #[memsize(scope = "body")]
        #[shrink(skip)]
        body: Option<TopSubtree>,
    },
    MacroDef {
        #[shrink(skip)]
        args: Option<TopSubtree>,
        #[shrink(skip)]
        body: Option<TopSubtree>,
    },
}

impl MacroDefinitionPayload {
    fn from_item(item: &MacroDefinitionItem) -> Self {
        match item {
            MacroDefinitionItem::MacroRules { body, .. } => Self::MacroRules { body: body.clone() },
            MacroDefinitionItem::MacroDef { args, body } => Self::MacroDef {
                args: args.clone(),
                body: body.clone(),
            },
        }
    }
}

/// Borrowed macro-definition facts selected from a resolved `DefId`.
///
/// Macro resolution often starts from a scope binding, but expansion also needs the local
/// definition's module/source metadata and the retained token-tree payload. This view keeps those
/// borrowed pieces together without making every caller repeat the "is this really a macro"
/// check.
#[derive(Debug, Clone, Copy)]
pub struct MacroDefinitionView<'a> {
    /// Stable identity used for cache keys and duplicate-candidate collapse.
    pub def_ref: LocalDefRef,
    /// The ordinary local definition record that owns visibility, module, and source facts.
    pub local_def: &'a LocalDefData,
    /// Retained macro body, builtin identity, and edition data used by expansion.
    pub data: &'a MacroDefinitionData,
}

impl<'a> MacroDefinitionView<'a> {
    /// Build a view only when the local definition kind agrees with the retained macro payload.
    pub fn new(
        def_ref: LocalDefRef,
        local_def: &'a LocalDefData,
        data: &'a MacroDefinitionData,
    ) -> Option<Self> {
        if local_def.kind != LocalDefKind::MacroDefinition {
            return None;
        }

        Some(Self {
            def_ref,
            local_def,
            data,
        })
    }

    /// Projects retained DefMap data into the syntax-only input accepted by the macro runtime.
    ///
    /// Compiler builtins have no declarative implementation to compile, so callers must handle
    /// their builtin identity before asking for this view.
    pub fn declarative_definition(self) -> Option<DeclarativeMacroDefinition<'a>> {
        if self.data.builtin.is_some() {
            return None;
        }

        Some(match &self.data.payload {
            MacroDefinitionPayload::MacroRules { body } => DeclarativeMacroDefinition::MacroRules {
                edition: self.data.edition,
                body: body.as_ref(),
            },
            MacroDefinitionPayload::MacroDef { args, body } => {
                DeclarativeMacroDefinition::MacroDef {
                    edition: self.data.edition,
                    args: args.as_ref(),
                    body: body.as_ref(),
                }
            }
        })
    }
}

impl PartialEq for MacroDefinitionView<'_> {
    fn eq(&self, other: &Self) -> bool {
        // Candidate uniqueness is definition identity; local data is the expansion payload.
        if self.def_ref != other.def_ref {
            return false;
        }

        // Within one DefMap snapshot, one local-def ref should always point at the same borrowed
        // records. Keep the asserts here so equality can stay focused on candidate identity.
        debug_assert_eq!(self.local_def, other.local_def);
        debug_assert_eq!(self.data, other.data);

        true
    }
}

impl Eq for MacroDefinitionView<'_> {}

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
