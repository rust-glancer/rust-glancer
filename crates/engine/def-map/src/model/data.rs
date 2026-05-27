use rg_item_tree::{Documentation, ItemTag, MacroDefinitionItem, VisibilityLevel};
use rg_ir_model::{ImportId, LocalDefId, LocalImplId, ModuleId, TargetRef};
use rg_parse::{FileId, Span};
use rg_text::Name;
use rg_tt::TopSubtree;
use rg_workspace::RustEdition;

use super::scope::Namespace;
use super::{ItemSource, ModuleScope};

/// One module in the frozen namespace graph.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct ModuleData {
    pub name: Option<Name>,
    pub name_span: Option<Span>,
    pub docs: Option<Documentation>,
    pub parent: Option<ModuleId>,
    pub children: Vec<(Name, ModuleId)>,
    pub local_defs: Vec<LocalDefId>,
    pub impls: Vec<LocalImplId>,
    pub imports: Vec<ImportId>,
    pub unresolved_imports: Vec<ImportId>,
    pub scope: ModuleScope,
    pub origin: ModuleOrigin,
}

impl ModuleData {
    pub(crate) fn shrink_to_fit(&mut self) {
        if let Some(name) = &mut self.name {
            name.shrink_to_fit();
        }
        if let Some(docs) = &mut self.docs {
            docs.shrink_to_fit();
        }
        self.children.shrink_to_fit();
        for (name, _) in &mut self.children {
            name.shrink_to_fit();
        }
        self.local_defs.shrink_to_fit();
        self.impls.shrink_to_fit();
        self.imports.shrink_to_fit();
        self.unresolved_imports.shrink_to_fit();
        self.scope.shrink_to_fit();
    }
}

/// Where a module came from in source code.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub enum ModuleOrigin {
    Root {
        file_id: FileId,
    },
    Inline {
        declaration_file: FileId,
        declaration_span: Span,
    },
    OutOfLine {
        declaration_file: FileId,
        declaration_span: Span,
        definition_file: Option<FileId>,
    },
}

impl ModuleOrigin {
    /// Returns whether this module's source touches the requested file.
    pub fn contains_file(&self, file_id: FileId) -> bool {
        match self {
            Self::Root { file_id: root_file } => *root_file == file_id,
            Self::Inline {
                declaration_file, ..
            } => *declaration_file == file_id,
            Self::OutOfLine {
                declaration_file,
                definition_file,
                ..
            } => *declaration_file == file_id || *definition_file == Some(file_id),
        }
    }
}

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
    pub(crate) fn from_item(
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
    pub(crate) fn from_item_tag(tag: ItemTag) -> Option<Self> {
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

    pub(crate) fn namespace(self) -> Namespace {
        match self {
            Self::Const | Self::Function | Self::Static => Namespace::Values,
            Self::Enum | Self::Struct | Self::Trait | Self::TypeAlias | Self::Union => {
                Namespace::Types
            }
            Self::MacroDefinition => Namespace::Macros,
        }
    }
}
