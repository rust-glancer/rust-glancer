use rg_ir_model::{ModuleId, TargetRef, hir::source::ItemSource};
use rg_item_tree::{ItemTag, MacroDefinitionItem, VisibilityLevel};
use rg_parse::{FileId, Span};
use rg_text::Name;
use rg_tt::TopSubtree;
use rg_workspace::RustEdition;

use super::scope::Namespace;

/// One module-scope definition collected from source.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
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

impl LocalDefData {
    pub(crate) fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
    }
}

/// Declarative macro definition payload retained for expansion after def-map freezing.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct MacroDefinitionData {
    pub edition: RustEdition,
    /// Target that `$crate` inside this macro body should resolve to when expanded.
    pub dollar_crate_target: TargetRef,
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
            payload: MacroDefinitionPayload::from_item(item),
        }
    }

    pub(crate) fn shrink_to_fit(&mut self) {}
}

/// Token-tree payload needed to compile a collected declarative macro.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub enum MacroDefinitionPayload {
    MacroRules {
        #[memsize(scope = "body")]
        body: Option<TopSubtree>,
    },
    MacroDef {
        args: Option<TopSubtree>,
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

/// One module-owned impl block collected from source.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
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
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
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
