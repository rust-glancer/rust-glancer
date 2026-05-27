use std::fmt;

use rg_body_ir::{BodyItemKind, BodyValueItemKind};
use rg_def_map::Path;
use rg_ir_model::identity::{
    DeclarationRef, EnumVariantRef, ExprRef, FieldRef, FunctionBodyRef, FunctionRef,
    LexicalScopeRef,
};
use rg_ir_model::{ModuleRef, TargetRef, TraitApplicability};
use rg_parse::{FileId, Span};
use rg_semantic_ir::TypePathContext;

use rg_ir_view::SymbolKind;

/// Scope in which a type path should be resolved.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TypePathScopeRef(TypePathScopeRepr);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypePathScopeRepr {
    Signature(TypePathContext),
    Body(LexicalScopeRef),
}

impl TypePathScopeRef {
    pub(crate) fn signature(context: TypePathContext) -> Self {
        Self(TypePathScopeRepr::Signature(context))
    }

    pub(crate) fn body(scope: LexicalScopeRef) -> Self {
        Self(TypePathScopeRepr::Body(scope))
    }

    pub(crate) fn repr(self) -> TypePathScopeRepr {
        self.0
    }
}

impl fmt::Debug for TypePathScopeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            TypePathScopeRepr::Signature(context) => f
                .debug_struct("TypePathScopeRef")
                .field("kind", &"signature")
                .field("module", &context.module)
                .finish(),
            TypePathScopeRepr::Body(scope) => f
                .debug_struct("TypePathScopeRef")
                .field("kind", &"body")
                .field("scope", &scope)
                .finish(),
        }
    }
}

/// Symbol found at one source offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolAt {
    /// Function body declaration, e.g. the name in `fn use_it() { ... }`.
    FunctionBody { body: FunctionBodyRef },
    /// Declaration-like source node.
    Declaration {
        declaration: DeclarationRef,
        span: Span,
    },
    /// Lowered expression node, e.g. the whole `user.id()` call expression.
    Expr { expr: ExprRef },
    /// Type-namespace path, e.g. `User` in a signature or `let user: User;`.
    TypePath {
        scope: TypePathScopeRef,
        path: Path,
        span: Span,
    },
    /// Value-namespace path inside a lowered body.
    ValuePath {
        scope: LexicalScopeRef,
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

impl From<SymbolKind> for NavigationTargetKind {
    fn from(kind: SymbolKind) -> Self {
        match kind {
            SymbolKind::Const => Self::Const,
            SymbolKind::Enum => Self::Enum,
            SymbolKind::EnumVariant => Self::EnumVariant,
            SymbolKind::Field => Self::Field,
            SymbolKind::Function | SymbolKind::Method => Self::Function,
            SymbolKind::Impl => Self::Impl,
            SymbolKind::Macro => Self::Macro,
            SymbolKind::Module => Self::Module,
            SymbolKind::Static => Self::Static,
            SymbolKind::Struct => Self::Struct,
            SymbolKind::Trait => Self::Trait,
            SymbolKind::TypeAlias => Self::TypeAlias,
            SymbolKind::Union => Self::Union,
            SymbolKind::Variable => Self::LocalBinding,
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
    Declaration(DeclarationRef),
    EnumVariant(EnumVariantRef),
    Field(FieldRef),
    Function(FunctionRef),
    Keyword(KeywordCompletion),
    PrimitiveType(rg_ty::PrimitiveTy),
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
    #[display("primitive_type")]
    PrimitiveType,
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
            | Self::PrimitiveType
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
            Self::Struct | Self::Enum | Self::Union | Self::TypeAlias | Self::PrimitiveType => 0,
            Self::Trait => 1,
            Self::Module => 2,
            Self::Keyword => 3,
            _ => 4,
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
