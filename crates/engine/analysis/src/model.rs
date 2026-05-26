use std::fmt;

use rg_body_ir::{
    BodyBindingRef, BodyDeclarationRef, BodyEnumVariantRef, BodyFieldRef, BodyFunctionOwner,
    BodyFunctionRef, BodyItemKind, BodyItemRef, BodyRef as BodyIrBodyRef, BodyValueItemKind,
    BodyValueItemRef, ExprId, ScopeId,
};
use rg_def_map::{DefId, LocalDefKind, LocalDefRef, ModuleRef, Path, TargetRef};
use rg_parse::{FileId, Span};
use rg_semantic_ir::{
    EnumVariantRef as SemanticEnumVariantRef, FieldRef as SemanticFieldRef,
    FunctionRef as SemanticFunctionRef, SemanticDeclarationRef, SemanticItemKind,
    TraitApplicability, TypePathContext,
};

/// Stable identity for one lowered function body.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionBodyRef(BodyIrBodyRef);

impl FunctionBodyRef {
    #[cfg(test)]
    pub(crate) fn body_ir(self) -> BodyIrBodyRef {
        self.0
    }

    pub(crate) fn from_body_ir(body: BodyIrBodyRef) -> Self {
        Self(body)
    }

    pub fn target(self) -> TargetRef {
        self.0.target
    }
}

impl fmt::Debug for FunctionBodyRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FunctionBodyRef")
            .field("target", &self.0.target)
            .field("body", &self.0.body)
            .finish()
    }
}

/// Stable identity for one expression inside a lowered body.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExprRef {
    body: BodyIrBodyRef,
    expr: ExprId,
}

impl ExprRef {
    pub(crate) fn new(body: BodyIrBodyRef, expr: ExprId) -> Self {
        Self { body, expr }
    }

    pub(crate) fn body_ir(self) -> BodyIrBodyRef {
        self.body
    }

    pub(crate) fn expr_id(self) -> ExprId {
        self.expr
    }
}

impl fmt::Debug for ExprRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExprRef")
            .field("body", &FunctionBodyRef::from_body_ir(self.body))
            .field("expr", &self.expr)
            .finish()
    }
}

/// Stable identity for one lexical scope inside a lowered body.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct LexicalScopeRef {
    body: BodyIrBodyRef,
    scope: ScopeId,
}

impl LexicalScopeRef {
    pub(crate) fn new(body: BodyIrBodyRef, scope: ScopeId) -> Self {
        Self { body, scope }
    }

    pub(crate) fn body_ir(self) -> BodyIrBodyRef {
        self.body
    }

    pub(crate) fn scope_id(self) -> ScopeId {
        self.scope
    }
}

impl fmt::Debug for LexicalScopeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LexicalScopeRef")
            .field("body", &FunctionBodyRef::from_body_ir(self.body))
            .field("scope", &self.scope)
            .finish()
    }
}

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

/// Stable declaration identity exposed by analysis-facing features.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeclarationRef(DeclarationRefRepr);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DeclarationRefRepr {
    Module(ModuleRef),
    LocalDef(LocalDefRef),
    Semantic(SemanticDeclarationRef),
    Body(BodyDeclarationRef),
}

impl DeclarationRef {
    pub(crate) fn module(module: ModuleRef) -> Self {
        Self(DeclarationRefRepr::Module(module))
    }

    pub(crate) fn local_def(local_def: LocalDefRef) -> Self {
        Self(DeclarationRefRepr::LocalDef(local_def))
    }

    pub(crate) fn semantic(declaration: SemanticDeclarationRef) -> Self {
        Self(DeclarationRefRepr::Semantic(declaration))
    }

    pub(crate) fn body(declaration: BodyDeclarationRef) -> Self {
        Self(DeclarationRefRepr::Body(declaration))
    }

    pub(crate) fn from_def(def: DefId) -> Self {
        match def {
            DefId::Module(module) => Self::module(module),
            DefId::Local(local_def) => Self::local_def(local_def),
        }
    }

    pub(crate) fn binding(binding: BodyBindingRef) -> Self {
        Self::body(BodyDeclarationRef::Binding(binding))
    }

    pub(crate) fn body_item(item: BodyItemRef) -> Self {
        Self::body(BodyDeclarationRef::Item(item))
    }

    pub(crate) fn body_value_item(item: BodyValueItemRef) -> Self {
        Self::body(BodyDeclarationRef::ValueItem(item))
    }

    pub(crate) fn body_field(field: BodyFieldRef) -> Self {
        Self::body(BodyDeclarationRef::Field(field))
    }

    pub(crate) fn body_enum_variant(variant: BodyEnumVariantRef) -> Self {
        Self::body(BodyDeclarationRef::EnumVariant(variant))
    }

    pub(crate) fn body_function(function: BodyFunctionRef) -> Self {
        Self::body(BodyDeclarationRef::Function(function))
    }

    pub(crate) fn repr(self) -> DeclarationRefRepr {
        self.0
    }
}

impl fmt::Debug for DeclarationRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            DeclarationRefRepr::Module(module) => f
                .debug_struct("DeclarationRef")
                .field("kind", &"module")
                .field("module", &module)
                .finish(),
            DeclarationRefRepr::LocalDef(local_def) => f
                .debug_struct("DeclarationRef")
                .field("kind", &"local_def")
                .field("local_def", &local_def)
                .finish(),
            DeclarationRefRepr::Semantic(declaration) => f
                .debug_struct("DeclarationRef")
                .field("kind", &"signature")
                .field("declaration", &declaration)
                .finish(),
            DeclarationRefRepr::Body(declaration) => f
                .debug_struct("DeclarationRef")
                .field("kind", &"body")
                .field("declaration", &declaration)
                .finish(),
        }
    }
}

/// Stable field identity exposed by analysis-facing features.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FieldRef(FieldRefRepr);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FieldRefRepr {
    Semantic(SemanticFieldRef),
    BodyLocal(BodyFieldRef),
}

impl FieldRef {
    pub(crate) fn semantic(field: SemanticFieldRef) -> Self {
        Self(FieldRefRepr::Semantic(field))
    }

    pub(crate) fn body_local(field: BodyFieldRef) -> Self {
        Self(FieldRefRepr::BodyLocal(field))
    }

    pub(crate) fn repr(self) -> FieldRefRepr {
        self.0
    }
}

impl fmt::Debug for FieldRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            FieldRefRepr::Semantic(field) => f
                .debug_struct("FieldRef")
                .field("owner", &field.owner)
                .field("index", &field.index)
                .finish(),
            FieldRefRepr::BodyLocal(field) => f
                .debug_struct("FieldRef")
                .field("owner", &field.item)
                .field("index", &field.index)
                .finish(),
        }
    }
}

/// Stable function identity exposed by analysis-facing features.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionRef(FunctionRefRepr);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FunctionRefRepr {
    Semantic(SemanticFunctionRef),
    BodyLocal(BodyFunctionRef),
}

impl FunctionRef {
    pub(crate) fn semantic(function: SemanticFunctionRef) -> Self {
        Self(FunctionRefRepr::Semantic(function))
    }

    pub(crate) fn body_local(function: BodyFunctionRef) -> Self {
        Self(FunctionRefRepr::BodyLocal(function))
    }

    pub(crate) fn repr(self) -> FunctionRefRepr {
        self.0
    }
}

impl fmt::Debug for FunctionRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            FunctionRefRepr::Semantic(function) => f
                .debug_struct("FunctionRef")
                .field("target", &function.target)
                .field("function", &function.id)
                .finish(),
            FunctionRefRepr::BodyLocal(function) => f
                .debug_struct("FunctionRef")
                .field("body", &FunctionBodyRef::from_body_ir(function.body))
                .field("function", &function.function)
                .finish(),
        }
    }
}

/// Stable enum variant identity exposed by analysis-facing features.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumVariantRef(EnumVariantRefRepr);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum EnumVariantRefRepr {
    Semantic(SemanticEnumVariantRef),
    BodyLocal(BodyEnumVariantRef),
}

impl EnumVariantRef {
    pub(crate) fn semantic(variant: SemanticEnumVariantRef) -> Self {
        Self(EnumVariantRefRepr::Semantic(variant))
    }

    pub(crate) fn body_local(variant: BodyEnumVariantRef) -> Self {
        Self(EnumVariantRefRepr::BodyLocal(variant))
    }

    pub(crate) fn repr(self) -> EnumVariantRefRepr {
        self.0
    }
}

impl fmt::Debug for EnumVariantRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            EnumVariantRefRepr::Semantic(variant) => f
                .debug_struct("EnumVariantRef")
                .field("target", &variant.target)
                .field("enum", &variant.enum_id)
                .field("index", &variant.index)
                .finish(),
            EnumVariantRefRepr::BodyLocal(variant) => f
                .debug_struct("EnumVariantRef")
                .field("owner", &variant.item)
                .field("index", &variant.index)
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

    pub(super) fn from_semantic_item_kind(kind: SemanticItemKind) -> Self {
        match kind {
            SemanticItemKind::Struct => Self::Struct,
            SemanticItemKind::Enum => Self::Enum,
            SemanticItemKind::Union => Self::Union,
            SemanticItemKind::Trait => Self::Trait,
            SemanticItemKind::Impl => Self::Impl,
            SemanticItemKind::Function => Self::Function,
            SemanticItemKind::TypeAlias => Self::TypeAlias,
            SemanticItemKind::Const => Self::Const,
            SemanticItemKind::Static => Self::Static,
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
