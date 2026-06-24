use rg_ir_model::items::{BuiltinMacroKind, ItemTag, MacroDefinitionItem, VisibilityLevel};
use rg_ir_model::{LocalDefRef, ModuleId, TargetRef, hir::source::ItemSource};
use rg_parse::{FileId, Span};
use rg_std::{MemorySize, Shrink};
use rg_text::Name;
use rg_tt::TopSubtree;
use rg_workspace::RustEdition;
use wincode::{SchemaRead, SchemaWrite};

use super::scope::Namespace;

/// One module-scope definition collected from source.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct LocalDefData {
    pub module: ModuleId,
    pub name: Name,
    pub kind: LocalDefKind,
    pub visibility: VisibilityLevel,
    pub source: ItemSource,
    pub file_id: FileId,
    pub name_span: Option<Span>,
    pub span: Span,
}

/// Macro definition facts retained after def-map freezing.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct MacroDefinitionData {
    pub edition: RustEdition,
    /// Target that `$crate` inside this macro body should resolve to when expanded.
    pub dollar_crate_target: TargetRef,
    /// Compiler hook that should run instead of declarative expansion, if any.
    pub builtin: Option<BuiltinMacroKind>,
    #[shrink(skip)]
    pub payload: MacroDefinitionPayload,
}

impl MacroDefinitionData {
    pub fn from_item(
        item: &MacroDefinitionItem,
        edition: RustEdition,
        dollar_crate_target: TargetRef,
    ) -> Self {
        Self {
            edition,
            dollar_crate_target,
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

    pub fn namespace(self) -> Namespace {
        match self {
            Self::Const | Self::Function | Self::Static => Namespace::Values,
            Self::Enum | Self::Struct | Self::Trait | Self::TypeAlias | Self::Union => {
                Namespace::Types
            }
            Self::MacroDefinition => Namespace::Macros,
        }
    }
}
