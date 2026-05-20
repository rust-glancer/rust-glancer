use rg_body_ir::{
    BindingData, BindingId, BodyEnumVariantRef, BodyFieldRef, BodyFunctionOwner, BodyFunctionRef,
    BodyItemKind, BodyItemRef, BodyRef, BodyValueItemKind, BodyValueItemRef, ExprId,
    ResolvedEnumVariantRef, ResolvedFieldRef, ResolvedFunctionRef, ScopeId,
};
use rg_def_map::{DefId, LocalDefKind, ModuleRef, Path, TargetRef};
use rg_parse::{FileId, Span};
use rg_semantic_ir::{EnumVariantRef, FieldRef, FunctionRef, TraitApplicability, TypePathContext};

pub(super) struct SymbolCandidate {
    pub(super) symbol: SymbolAt,
    pub(super) span: Span,
}

/// Symbol found at one source offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolAt {
    /// Function body declaration, e.g. the name in `fn use_it() { ... }`.
    Body { body: BodyRef },
    /// Local binding introduced by a parameter or pattern, e.g. `user` in `let user = input;`.
    Binding { body: BodyRef, binding: BindingId },
    /// Body-local type path, e.g. `User` in `let user: User;`.
    BodyPath {
        body: BodyRef,
        scope: ScopeId,
        path: Path,
        span: Span,
    },
    /// Body-local value path, e.g. `DEFAULT` in `let value = DEFAULT;`.
    BodyValuePath {
        body: BodyRef,
        scope: ScopeId,
        path: Path,
        span: Span,
    },
    /// DefMap-backed declaration, e.g. a module-level `struct User;`.
    Def { def: DefId, span: Span },
    /// Lowered expression node, e.g. the whole `user.id()` call expression.
    Expr { body: BodyRef, expr: ExprId },
    /// Semantic field declaration, e.g. `id` in a module-level `struct User { id: Id }`.
    Field { field: FieldRef, span: Span },
    /// Semantic function declaration, e.g. `make_user` in `fn make_user() -> User`.
    Function { function: FunctionRef, span: Span },
    /// Semantic enum variant declaration, e.g. `Some` in `enum Maybe<T> { Some(T) }`.
    EnumVariant { variant: EnumVariantRef, span: Span },
    /// Variant declared on a body-local enum, e.g. `Start` in `enum Action { Start }`.
    LocalEnumVariant {
        variant: BodyEnumVariantRef,
        span: Span,
    },
    /// Body-local type-namespace item, e.g. `User` in `fn f() { struct User; }`.
    ///
    /// This also covers local `enum`, `union`, `type`, and `trait` declarations.
    LocalItem { item: BodyItemRef, span: Span },
    /// Body-local value-namespace item, e.g. `DEFAULT` in `fn f() { const DEFAULT: u8 = 0; }`.
    ///
    /// This covers local `const` and `static` declarations, not `let` bindings.
    LocalValueItem { item: BodyValueItemRef, span: Span },
    /// Field declared on a body-local struct or union,
    /// e.g. `id` in `fn f() { struct User { id: Id } }`.
    LocalField { field: BodyFieldRef, span: Span },
    /// Body-local function-like item, e.g. `helper` in `fn f() { fn helper() {} }`.
    LocalFunction {
        function: BodyFunctionRef,
        span: Span,
    },
    /// Semantic type path outside body IR, e.g. `User` in a function signature.
    TypePath {
        context: TypePathContext,
        path: Path,
        span: Span,
    },
    /// Import path, e.g. `crate::user::User` in `use crate::user::User;`.
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
            BodyItemKind::Enum => Self::Enum,
            BodyItemKind::Union => Self::Union,
            BodyItemKind::TypeAlias => Self::TypeAlias,
            BodyItemKind::Trait => Self::Trait,
        }
    }

    pub(super) fn from_body_value_item_kind(kind: BodyValueItemKind) -> Self {
        match kind {
            BodyValueItemKind::Const => Self::Const,
            BodyValueItemKind::Static => Self::Static,
        }
    }

    pub(super) fn from_body_function_owner(owner: BodyFunctionOwner) -> Self {
        match owner {
            BodyFunctionOwner::LocalScope(_) => Self::Function,
            BodyFunctionOwner::LocalImpl(_) => Self::Method,
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
            BodyItemKind::Enum => Self::Enum,
            BodyItemKind::Union => Self::Union,
            BodyItemKind::TypeAlias => Self::TypeAlias,
            BodyItemKind::Trait => Self::Trait,
        }
    }

    pub(super) fn from_body_value_item_kind(kind: BodyValueItemKind) -> Self {
        match kind {
            BodyValueItemKind::Const => Self::Const,
            BodyValueItemKind::Static => Self::Static,
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
    pub detail: Option<String>,
    pub documentation: Option<String>,
    pub sort_text: String,
    pub insert_text: CompletionInsertText,
    pub edit: Option<CompletionEdit>,
}

/// Text inserted when accepting a completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionInsertText {
    Plain,
    Snippet(String),
}

/// Source edit applied when accepting a completion item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletionEdit {
    pub replace: Span,
}

/// Stable analysis identity behind one completion row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionTarget {
    Binding { body: BodyRef, binding: BindingId },
    BodyItem(BodyItemRef),
    BodyValueItem(BodyValueItemRef),
    EnumVariant(ResolvedEnumVariantRef),
    Field(ResolvedFieldRef),
    Function(ResolvedFunctionRef),
    Def(DefId),
    Keyword(KeywordCompletion),
}

/// Small, explicit set of Rust keyword and keyword-like snippet completions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeywordCompletion {
    Async,
    Const,
    Enum,
    False,
    Fn,
    For,
    If,
    Impl,
    ImplFor,
    Let,
    Loop,
    Match,
    Mod,
    Move,
    Return,
    Static,
    Struct,
    Trait,
    True,
    Type,
    Use,
    While,
}

/// Completion source category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, derive_more::Display)]
pub enum CompletionKind {
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
    #[display("inherent_method")]
    InherentMethod,
    #[display("keyword")]
    Keyword,
    #[display("macro")]
    Macro,
    #[display("module")]
    Module,
    #[display("static")]
    Static,
    #[display("struct")]
    Struct,
    #[display("trait")]
    Trait,
    #[display("trait_method")]
    TraitMethod,
    #[display("type_alias")]
    TypeAlias,
    #[display("union")]
    Union,
    #[display("variable")]
    Variable,
}

impl CompletionKind {
    /// Coarse bucket used as one component of LSP `sortText`.
    ///
    /// This is not the enum's full ordering: some variants intentionally share a
    /// bucket, and completion ordering also includes label, applicability, and
    /// target identity. Derived `Ord` remains the ordinary total enum order.
    pub(super) fn sort_text_rank(self) -> u8 {
        match self {
            Self::Field => 0,
            Self::InherentMethod => 1,
            Self::TraitMethod => 2,
            Self::Module => 3,
            Self::Struct
            | Self::Enum
            | Self::EnumVariant
            | Self::Trait
            | Self::TypeAlias
            | Self::Union => 4,
            Self::Const | Self::Static => 5,
            Self::Function | Self::Macro => 6,
            Self::Variable => 7,
            Self::Keyword => 8,
        }
    }

    /// Coarse bucket used by type-position completions that can still accept modules as prefixes.
    ///
    /// This is a context-specific component of LSP `sortText`, not the enum's general ordering.
    pub(super) fn type_context_sort_text_rank(self) -> u8 {
        match self {
            Self::Struct | Self::Enum | Self::Union | Self::TypeAlias => 0,
            Self::Trait => 1,
            Self::Module => 2,
            Self::Keyword => 3,
            _ => 4,
        }
    }

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
            BodyItemKind::Enum => Self::Enum,
            BodyItemKind::Union => Self::Union,
            BodyItemKind::TypeAlias => Self::TypeAlias,
            BodyItemKind::Trait => Self::Trait,
        }
    }

    pub(super) fn from_body_value_item_kind(kind: BodyValueItemKind) -> Self {
        match kind {
            BodyValueItemKind::Const => Self::Const,
            BodyValueItemKind::Static => Self::Static,
        }
    }
}

/// Confidence attached to a completion candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, derive_more::Display)]
pub enum CompletionApplicability {
    #[display("known")]
    Known,
    #[display("maybe")]
    Maybe,
}

impl CompletionApplicability {
    /// Coarse bucket used as one component of LSP `sortText`.
    ///
    /// This is not the completion item's full ordering: applicability is only
    /// one part of the final sort key. Derived `Ord` remains the ordinary total
    /// enum order.
    pub(super) fn sort_text_rank(self) -> u8 {
        match self {
            Self::Known => 0,
            Self::Maybe => 1,
        }
    }
}

impl From<TraitApplicability> for CompletionApplicability {
    fn from(applicability: TraitApplicability) -> Self {
        match applicability {
            TraitApplicability::Yes => Self::Known,
            TraitApplicability::Maybe | TraitApplicability::No => Self::Maybe,
        }
    }
}
