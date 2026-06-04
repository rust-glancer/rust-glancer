use rg_ir_model::{
    EnumVariantRef, FieldRef, FunctionRef, SemanticItemKind, TraitApplicability,
    identity::DeclarationRef,
};
use rg_parse::Span;

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
    pub(crate) fn sort_text_rank(self) -> u8 {
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
    pub(crate) fn type_context_sort_text_rank(self) -> u8 {
        match self {
            Self::Struct | Self::Enum | Self::Union | Self::TypeAlias | Self::PrimitiveType => 0,
            Self::Trait => 1,
            Self::Module => 2,
            Self::Keyword => 3,
            _ => 4,
        }
    }

    pub(crate) fn from_semantic_item_kind(kind: SemanticItemKind) -> Option<Self> {
        Some(match kind {
            SemanticItemKind::Struct => Self::Struct,
            SemanticItemKind::Enum => Self::Enum,
            SemanticItemKind::Union => Self::Union,
            SemanticItemKind::Trait => Self::Trait,
            SemanticItemKind::Function => Self::Function,
            SemanticItemKind::TypeAlias => Self::TypeAlias,
            SemanticItemKind::Const => Self::Const,
            SemanticItemKind::Static => Self::Static,
            SemanticItemKind::Impl => return None,
        })
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
    pub(crate) fn sort_text_rank(self) -> u8 {
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
