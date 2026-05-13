use rg_body_ir::{
    BindingData, BindingId, BodyFieldRef, BodyFunctionRef, BodyItemKind, BodyItemRef, BodyRef,
    ExprId, ResolvedFieldRef, ResolvedFunctionRef, ScopeId,
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
    LocalField {
        field: BodyFieldRef,
        span: Span,
    },
    LocalFunction {
        function: BodyFunctionRef,
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

/// One source occurrence of the declaration-like subject selected by a references query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceLocation {
    pub target: TargetRef,
    pub file_id: FileId,
    pub span: Span,
}

/// Options for a source reference lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceQuery<'a> {
    search_scope: ReferenceSearchScope<'a>,
    declaration_policy: ReferenceDeclarationPolicy,
}

impl<'a> ReferenceQuery<'a> {
    /// Returns a query for explicit find-references requests.
    pub fn find_references(search_targets: &'a [TargetRef], include_declarations: bool) -> Self {
        let declaration_policy = if include_declarations {
            ReferenceDeclarationPolicy::IncludeUnscoped
        } else {
            ReferenceDeclarationPolicy::Exclude
        };

        Self {
            search_scope: ReferenceSearchScope::Targets(search_targets),
            declaration_policy,
        }
    }

    /// Returns a query scoped to one file inside one target.
    pub fn file_scoped(target: TargetRef, file_id: FileId) -> Self {
        Self {
            search_scope: ReferenceSearchScope::File { target, file_id },
            declaration_policy: ReferenceDeclarationPolicy::IncludeInSearchScope,
        }
    }

    /// Removes declaration locations from this query.
    pub fn without_declarations(mut self) -> Self {
        self.declaration_policy = ReferenceDeclarationPolicy::Exclude;
        self
    }

    pub(crate) fn search_scope(self) -> ReferenceSearchScope<'a> {
        self.search_scope
    }

    pub(crate) fn includes_declarations(self) -> bool {
        !matches!(self.declaration_policy, ReferenceDeclarationPolicy::Exclude)
    }

    pub(crate) fn accepts_declaration(self, target: TargetRef, file_id: FileId) -> bool {
        match self.declaration_policy {
            ReferenceDeclarationPolicy::Exclude => false,
            ReferenceDeclarationPolicy::IncludeUnscoped => true,
            ReferenceDeclarationPolicy::IncludeInSearchScope => match self.search_scope {
                ReferenceSearchScope::Targets(targets) => targets.contains(&target),
                ReferenceSearchScope::File {
                    target: selected_target,
                    file_id: selected_file_id,
                } => selected_target == target && selected_file_id == file_id,
            },
        }
    }
}

/// Source surface scanned for reference use-sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReferenceSearchScope<'a> {
    /// Scans all source candidates inside the listed targets.
    Targets(&'a [TargetRef]),
    /// Scans source candidates in one file inside one target.
    File { target: TargetRef, file_id: FileId },
}

/// How declaration locations should relate to the reference search surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReferenceDeclarationPolicy {
    /// Do not return declaration locations.
    Exclude,
    /// Return declarations even when they are outside `ReferenceSearchScope`.
    IncludeUnscoped,
    /// Return declarations only when they are inside `ReferenceSearchScope`.
    IncludeInSearchScope,
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
    #[display("impl")]
    Impl,
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
