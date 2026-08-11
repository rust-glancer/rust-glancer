//! Completion rows and source edits before protocol conversion.

use rg_ir_model::{
    EnumVariantFieldRef, EnumVariantRef, FieldRef, FunctionRef, GenericParamRef, ImplRef,
    PrimitiveTy, SemanticItemKind, TraitApplicability, identity::DeclarationRef,
};
use rg_parse::Span;

/// One completion row produced from a frozen analysis snapshot.
///
/// `target` retains semantic identity for navigation and stable ordering, while `insert_text`,
/// `edit`, and `additional_edits` describe what accepting the row does to source. Keeping those
/// concerns separate lets a displayed label such as `required` replace a larger prefix such as
/// `fn re`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    /// Name shown as the completion row's primary label.
    pub label: String,
    /// Text used by the client to filter this row when the replacement span includes syntax
    /// beyond the partial name. For an edit replacing `fn re`, this must be `fn required` rather
    /// than only `required`, because the client matches the complete replaced source prefix.
    pub filter_text: Option<String>,
    /// Presentation category used for editor icons and context-specific ordering.
    pub kind: CompletionKind,
    /// Semantic or synthetic identity retained independently from the displayed text.
    pub target: CompletionTarget,
    /// Whether semantic lookup proved that the candidate applies at this site.
    pub applicability: CompletionApplicability,
    /// Compact signature or category text displayed beside the label.
    pub detail: Option<String>,
    /// Declaration documentation before client-specific markup conversion.
    pub documentation: Option<String>,
    /// Precomputed lexicographic ordering key for the editor client.
    pub sort_text: String,
    /// Text policy used inside the primary replacement.
    pub insert_text: CompletionInsertText,
    /// Primary source range replaced when the row is accepted.
    pub edit: Option<CompletionEdit>,
    /// Non-overlapping source edits applied together with the primary completion replacement.
    pub additional_edits: Vec<CompletionAdditionalEdit>,
}

/// Text inserted when accepting a completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionInsertText {
    /// Insert the displayed completion label as ordinary text.
    Plain,
    /// Plain replacement text that intentionally differs from the displayed label.
    Text(String),
    /// Insert client-interpreted snippet text with placeholders and a final cursor position.
    Snippet(String),
}

/// Source edit applied when accepting a completion item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletionEdit {
    /// Source range replaced by the completion's insertion text.
    pub replace: Span,
}

/// One plain-text source edit attached to a completion acceptance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionAdditionalEdit {
    /// Source range changed separately from the primary completion edit.
    pub replace: Span,
    /// Ordinary source text written into `replace`.
    pub new_text: String,
}

/// Stable analysis identity behind one completion row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionTarget {
    /// A module, item, or body-local declaration covered by the shared declaration vocabulary.
    Declaration(DeclarationRef),
    /// An enum variant indexed below its owning enum declaration.
    EnumVariant(EnumVariantRef),
    /// A named or tuple field indexed below its owning type.
    Field(FieldRef),
    /// A named or tuple field indexed below one enum variant.
    EnumVariantField(EnumVariantFieldRef),
    /// A free, associated, or receiver function with function-specific semantic identity.
    Function(FunctionRef),
    /// A written type or const parameter with its stable semantic identity.
    GenericParam(GenericParamRef),
    /// `Self` introduced by an impl, whose target is the impl rather than a generic parameter.
    ImplSelf(ImplRef),
    /// A language keyword or keyword-shaped snippet rather than a source declaration.
    Keyword(KeywordCompletion),
    /// A builtin primitive type rather than a source declaration.
    PrimitiveType(PrimitiveTy),
    /// A request-scoped candidate that does not have an indexed source declaration.
    Synthetic(SyntheticCompletionTarget),
}

/// Stable categories for completion rows synthesized from source-adjacent state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntheticCompletionTarget {
    /// A conventional sibling file offered for an incomplete `mod name;` declaration.
    ModuleDeclaration,
    /// A built-in attribute, derive, lint, or attribute argument without a source declaration.
    Attribute,
    /// A language-owned lifetime such as `'static` or a request-local binder lifetime.
    Lifetime,
    /// An enclosing or newly declared loop label.
    Label,
    /// A language/tooling-owned token such as an ABI, Cargo environment name, or visibility root.
    SpecializedValue,
    /// A `macro_rules!` matcher fragment such as `expr` or `pat`.
    MacroFragment,
    /// A source transformation offered after a dot, such as `.box` or `.if`.
    Postfix,
}

/// Small, explicit set of Rust keyword and keyword-like snippet completions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeywordCompletion {
    Async,
    Const,
    Crate,
    Dyn,
    Enum,
    Extern,
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
    Mut,
    Pub,
    Ref,
    Return,
    Static,
    Struct,
    SelfValue,
    Super,
    Trait,
    True,
    Type,
    Union,
    Unsafe,
    Use,
    While,
}

/// Completion source category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, derive_more::Display)]
pub enum CompletionKind {
    #[display("attribute")]
    Attribute,
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
    #[display("label")]
    Label,
    #[display("lifetime")]
    Lifetime,
    #[display("macro")]
    Macro,
    #[display("module")]
    Module,
    #[display("primitive_type")]
    PrimitiveType,
    #[display("postfix")]
    Postfix,
    #[display("static")]
    Static,
    #[display("struct")]
    Struct,
    #[display("trait")]
    Trait,
    #[display("trait_method")]
    TraitMethod,
    #[display("type_parameter")]
    TypeParameter,
    #[display("type_alias")]
    TypeAlias,
    #[display("union")]
    Union,
    #[display("variable")]
    Variable,
    #[display("value")]
    Value,
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
            | Self::TypeParameter
            | Self::PrimitiveType
            | Self::TypeAlias
            | Self::Union => 4,
            Self::Const | Self::Static => 5,
            Self::Function | Self::Macro => 6,
            Self::Variable | Self::Lifetime | Self::Label => 7,
            Self::Attribute | Self::Value => 8,
            Self::Postfix => 9,
            Self::Keyword => 10,
        }
    }

    /// Coarse bucket used by type-position completions that can still accept modules as prefixes.
    ///
    /// This is a context-specific component of LSP `sortText`, not the enum's general ordering.
    pub(crate) fn type_context_sort_text_rank(self) -> u8 {
        match self {
            Self::Struct
            | Self::Enum
            | Self::Union
            | Self::TypeAlias
            | Self::TypeParameter
            | Self::PrimitiveType => 0,
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
    /// Semantic lookup established that the candidate applies at this site.
    #[display("known")]
    Known,
    /// The candidate is useful, but semantic lookup could not prove that it applies.
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
