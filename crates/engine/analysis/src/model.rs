use rg_body_ir::{
    BindingData, BindingId, BodyItemKind, BodyItemRef, BodyRef, ExprId, ResolvedFieldRef,
    ResolvedFunctionRef, ScopeId,
};
use rg_def_map::{DefId, LocalDefKind, ModuleRef, Path, TargetRef};
use rg_parse::{FileId, Span};
use rg_semantic_ir::{EnumVariantRef, FieldRef, FunctionRef, TypePathContext};

pub(super) struct SymbolCandidate {
    pub(super) symbol: SymbolAt,
    pub(super) span: Span,
}

/// Symbol found at one source offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolAt {
    Body {
        body: BodyRef,
    },
    Binding {
        body: BodyRef,
        binding: BindingId,
    },
    BodyPath {
        body: BodyRef,
        scope: ScopeId,
        path: Path,
        span: Span,
    },
    BodyValuePath {
        body: BodyRef,
        scope: ScopeId,
        path: Path,
        span: Span,
    },
    Def {
        def: DefId,
        span: Span,
    },
    Expr {
        body: BodyRef,
        expr: ExprId,
    },
    Field {
        field: FieldRef,
        span: Span,
    },
    Function {
        function: FunctionRef,
        span: Span,
    },
    EnumVariant {
        variant: EnumVariantRef,
        span: Span,
    },
    LocalItem {
        item: BodyItemRef,
        span: Span,
    },
    TypePath {
        context: TypePathContext,
        path: Path,
        span: Span,
    },
    UsePath {
        module: ModuleRef,
        path: Path,
        span: Span,
    },
}

/// One goto-definition destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationTarget {
    pub target: TargetRef,
    pub kind: NavigationTargetKind,
    pub name: String,
    pub file_id: FileId,
    pub span: Option<Span>,
}

impl NavigationTarget {
    pub(super) fn from_binding(target: TargetRef, binding: &BindingData) -> Self {
        Self {
            target,
            kind: NavigationTargetKind::LocalBinding,
            name: binding
                .name
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "<unsupported>".to_string()),
            file_id: binding.source.file_id,
            span: Some(binding.source.span),
        }
    }
}

/// Hierarchical source outline for one file under one target context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub file_id: FileId,
    pub span: Span,
    pub selection_span: Span,
    pub children: Vec<DocumentSymbol>,
}

/// Flat symbol row suitable for workspace-wide search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSymbol {
    pub target: TargetRef,
    pub name: String,
    pub kind: SymbolKind,
    pub file_id: FileId,
    pub span: Option<Span>,
    pub container_name: Option<String>,
}

/// One best-effort inferred type annotation suitable for editor inlay hints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeHint {
    pub file_id: FileId,
    pub span: Span,
    pub label: String,
}

/// Markdown-ready hover payload independent from LSP transport types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverInfo {
    pub range: Option<Span>,
    pub blocks: Vec<HoverBlock>,
}

/// One independently rendered hover section.
///
/// A single cursor position can resolve to several useful facts, such as a field shorthand that
/// refers both to a local variable and a field declaration. Keeping blocks separate lets clients
/// render those facts with clear separators without losing their individual symbol categories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverBlock {
    pub kind: SymbolKind,
    pub path: Option<String>,
    pub signature: Option<String>,
    pub ty: Option<String>,
    pub docs: Option<String>,
}

/// LSP-shaped symbol category without depending on LSP transport types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, derive_more::Display)]
pub enum SymbolKind {
    #[display("const")]
    Const,
    #[display("enum")]
    Enum,
    #[display("variant")]
    EnumVariant,
    #[display("field")]
    Field,
    #[display("fn")]
    Function,
    #[display("impl")]
    Impl,
    #[display("macro")]
    Macro,
    #[display("method")]
    Method,
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
    #[display("variable")]
    Variable,
}

impl SymbolKind {
    pub(super) fn from_local_def_kind(kind: LocalDefKind) -> Self {
        match kind {
            LocalDefKind::Const => Self::Const,
            LocalDefKind::Enum => Self::Enum,
            LocalDefKind::Function => Self::Function,
            LocalDefKind::MacroDefinition => Self::Macro,
            LocalDefKind::Static => Self::Static,
            LocalDefKind::Struct => Self::Struct,
            LocalDefKind::Trait => Self::Trait,
            LocalDefKind::TypeAlias => Self::TypeAlias,
            LocalDefKind::Union => Self::Union,
        }
    }

    pub(super) fn from_body_item_kind(kind: BodyItemKind) -> Self {
        match kind {
            BodyItemKind::Struct => Self::Struct,
        }
    }
}

/// Navigation target category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, derive_more::Display)]
pub enum NavigationTargetKind {
    #[display("local")]
    LocalBinding,
    #[display("module")]
    Module,
    #[display("const")]
    Const,
    #[display("enum")]
    Enum,
    #[display("variant")]
    EnumVariant,
    #[display("field")]
    Field,
    #[display("fn")]
    Function,
    #[display("macro")]
    Macro,
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

impl NavigationTargetKind {
    pub(super) fn from_local_def_kind(kind: LocalDefKind) -> Self {
        match kind {
            LocalDefKind::Const => Self::Const,
            LocalDefKind::Enum => Self::Enum,
            LocalDefKind::Function => Self::Function,
            LocalDefKind::MacroDefinition => Self::Macro,
            LocalDefKind::Static => Self::Static,
            LocalDefKind::Struct => Self::Struct,
            LocalDefKind::Trait => Self::Trait,
            LocalDefKind::TypeAlias => Self::TypeAlias,
            LocalDefKind::Union => Self::Union,
        }
    }

    pub(super) fn from_body_item_kind(kind: BodyItemKind) -> Self {
        match kind {
            BodyItemKind::Struct => Self::Struct,
        }
    }
}

/// One completion item produced from the current frozen analysis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    pub target: CompletionTarget,
    pub applicability: CompletionApplicability,
}

/// Stable analysis identity behind one completion row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionTarget {
    Field(ResolvedFieldRef),
    Function(ResolvedFunctionRef),
}

/// Completion source category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, derive_more::Display)]
pub enum CompletionKind {
    #[display("field")]
    Field,
    #[display("inherent_method")]
    InherentMethod,
    #[display("trait_method")]
    TraitMethod,
}

/// Confidence attached to a completion candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, derive_more::Display)]
pub enum CompletionApplicability {
    #[display("known")]
    Known,
    #[display("maybe")]
    Maybe,
}
