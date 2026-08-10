//! Completion-domain cursor sites.
//!
//! Indexed source views know where body expressions, declaration signatures, imports, and item
//! owners came from. The request buffer knows what the user has typed since the last complete Rust
//! construct. This module joins those two sources and exposes one completion-owned vocabulary to
//! candidate lookup.
//!
//! The detector first uses decisive request syntax such as `.`, `::`, `mod name`, or an incomplete
//! trait member. It then asks the matching indexed scanner for semantic scope and identity. Narrow
//! syntax-only grammars such as attributes and strings stay in `SyntaxCompletionContext` and do
//! not pretend to have an indexed source site.

use anyhow::Context as _;
use rg_ir_model::{CrateRef, Path};
use rg_parse::{FileId, Span};

use rg_ir_view::{
    IndexedViewDb,
    source::{
        IndexedAssociatedTypeBindingSite, IndexedMemberAccessSite, IndexedModuleSourceSite,
        IndexedPatternCompletionKind, IndexedQualifiedPathContext, IndexedQualifiedPathScope,
        IndexedQualifiedPathSite, IndexedRecordFieldListSite, IndexedSignatureTypeSite,
        IndexedTraitImplSite, IndexedTypeNamePosition, IndexedUnqualifiedNameContext,
        IndexedUnqualifiedNameScope, IndexedUnqualifiedNameSite, SourceCompletionView,
    },
};

/// One normalized syntax family selected for the cursor.
///
/// ```text
/// value.na$0                      -> Dot
/// model::Us$0                     -> Path
/// let _: Us$0                     -> Unqualified
/// User { na$0 }                   -> RecordField
/// T: Iterator<It$0 = u8>          -> AssociatedTypeBinding
/// impl Service for Worker { fn r$0 } -> TraitImpl
/// mod pars$0;                     -> ModuleDeclaration
/// build_it$0!();                  -> ModuleMacro
/// ```
pub(crate) enum CompletionSite {
    AssociatedTypeBinding(IndexedAssociatedTypeBindingSite),
    Dot(DotCompletionSite),
    ModuleDeclaration(ModuleDeclarationCompletionSite),
    ModuleMacro(ModuleMacroCompletionSite),
    Path(PathCompletionSite),
    TraitImpl(TraitImplCompletionSite),
    Unqualified(UnqualifiedCompletionSite),
    RecordField(RecordFieldCompletionSite),
}

impl CompletionSite {
    fn from_signature_type_site(site: IndexedSignatureTypeSite) -> Self {
        match site {
            IndexedSignatureTypeSite::AssociatedTypeBinding(site) => {
                Self::AssociatedTypeBinding(site)
            }
            IndexedSignatureTypeSite::Qualified(site) => Self::Path(PathCompletionSite::new(site)),
            IndexedSignatureTypeSite::Unqualified(site) => {
                Self::Unqualified(UnqualifiedCompletionSite::new(site))
            }
        }
    }
}

/// Request-local syntax facts used to choose and repair an indexed completion site.
///
/// Some fields are only routing hints, such as whether the cursor follows `.`. Others preserve
/// information that cannot exist in lowered IR yet, such as the empty qualifier in `Type::$0`, the
/// owner spelling in `Type { $0`, or the full `fn re` replacement span of an incomplete trait
/// member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletionSiteSyntax {
    inside_use_item: bool,
    after_dot: bool,
    after_colon_colon: bool,
    empty_qualified_path: Option<QualifiedPathCompletionSyntax>,
    empty_path: Option<EmptyPathCompletionContext>,
    empty_record_owner: Option<Path>,
    body_owner_start: Option<u32>,
    standalone: Option<StandaloneCompletionSiteSyntax>,
    member_prefix_span: Span,
    member_prefix: String,
}

impl CompletionSiteSyntax {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        inside_use_item: bool,
        after_dot: bool,
        after_colon_colon: bool,
        empty_qualified_path: Option<QualifiedPathCompletionSyntax>,
        empty_path: Option<EmptyPathCompletionContext>,
        empty_record_owner: Option<Path>,
        body_owner_start: Option<u32>,
        standalone: Option<StandaloneCompletionSiteSyntax>,
        member_prefix_span: Span,
        member_prefix: String,
    ) -> Self {
        Self {
            inside_use_item,
            after_dot,
            after_colon_colon,
            empty_qualified_path,
            empty_path,
            empty_record_owner,
            body_owner_start,
            standalone,
            member_prefix_span,
            member_prefix,
        }
    }
}

/// Syntax-owned completion sites whose source position does not lower into a path or expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StandaloneCompletionSiteSyntax {
    /// An incomplete declaration inside a resolved trait implementation.
    TraitImpl(TraitImplCompletionSyntax),
    /// A macro callee in expression/body scope, such as `tools::build$0!()`.
    BodyMacro { qualifier: Option<Path> },
    /// A macro callee in an item list, such as `tools::generate$0!();`.
    ModuleMacro { qualifier: Option<Path> },
    /// An out-of-line module declaration, such as `mod pars$0;`.
    ModuleDeclaration { has_path_attribute: bool },
}

/// Request-local declaration prefix for a possible missing trait member.
///
/// A bare `re$0` has no member kind and replaces only `re`. Once the user has written
/// `fn re$0`, the semantic family is known and accepting the scaffold must replace the whole
/// `fn re` prefix rather than leaving the written `fn` in front of the generated signature. The
/// lookup prefix retains `fn ` so the editor can still match the broader replacement text against
/// a candidate such as `fn required`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TraitImplCompletionSyntax {
    owner_start: u32,
    member_kind: Option<TraitImplMemberKind>,
    replace_span: Span,
    lookup_prefix: Option<String>,
}

impl TraitImplCompletionSyntax {
    pub(crate) fn new(
        owner_start: u32,
        member_kind: Option<TraitImplMemberKind>,
        replace_span: Span,
        lookup_prefix: Option<String>,
    ) -> Self {
        Self {
            owner_start,
            member_kind,
            replace_span,
            lookup_prefix,
        }
    }

    #[cfg(test)]
    pub(crate) fn member_kind(&self) -> Option<TraitImplMemberKind> {
        self.member_kind
    }

    #[cfg(test)]
    pub(crate) fn replace_span(&self) -> Span {
        self.replace_span
    }

    #[cfg(test)]
    pub(crate) fn lookup_prefix(&self) -> Option<&str> {
        self.lookup_prefix.as_deref()
    }
}

/// Associated declaration family already selected by a written item introducer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TraitImplMemberKind {
    Function,
    TypeAlias,
    Const,
}

/// A resolved trait implementation paired with the incomplete member prefix from the request.
///
/// The indexed site supplies the trait and impl identities. Request syntax supplies facts that do
/// not lower until the declaration is complete: selected member kind, replacement range, and the
/// lookup prefix used by the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TraitImplCompletionSite {
    source: IndexedTraitImplSite,
    member_kind: Option<TraitImplMemberKind>,
    replace_span: Span,
    lookup_prefix: Option<String>,
}

impl TraitImplCompletionSite {
    fn new(
        source: IndexedTraitImplSite,
        member_kind: Option<TraitImplMemberKind>,
        replace_span: Span,
        lookup_prefix: Option<String>,
    ) -> Self {
        Self {
            source,
            member_kind,
            replace_span,
            lookup_prefix,
        }
    }

    pub(crate) fn source(&self) -> IndexedTraitImplSite {
        self.source
    }

    pub(crate) fn member_kind(&self) -> Option<TraitImplMemberKind> {
        self.member_kind
    }

    pub(crate) fn replace_span(&self) -> Span {
        self.replace_span
    }

    /// Builds the client lookup key for an edit that replaces a declaration introducer.
    ///
    /// The editor filters against the entire replacement prefix, so `fn re` must be matched by
    /// `fn required`, not only by the displayed label `required`.
    pub(crate) fn filter_text(&self, label: &str) -> Option<String> {
        self.lookup_prefix
            .as_ref()
            .map(|prefix| format!("{prefix}{label}"))
    }
}

/// Item-list macro completion anchored to the semantic module containing the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModuleMacroCompletionSite {
    source: IndexedModuleSourceSite,
    qualifier: Option<Path>,
    member_prefix_span: Span,
}

impl ModuleMacroCompletionSite {
    fn new(
        source: IndexedModuleSourceSite,
        qualifier: Option<Path>,
        member_prefix_span: Span,
    ) -> Self {
        Self {
            source,
            qualifier,
            member_prefix_span,
        }
    }

    pub(crate) fn source(&self) -> &IndexedModuleSourceSite {
        &self.source
    }

    pub(crate) fn qualifier(&self) -> Option<&Path> {
        self.qualifier.as_ref()
    }

    pub(crate) fn replace_span(&self) -> Span {
        self.member_prefix_span
    }
}

/// Filesystem module-name completion anchored to the containing module source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModuleDeclarationCompletionSite {
    source: IndexedModuleSourceSite,
    has_path_attribute: bool,
    member_prefix_span: Span,
}

impl ModuleDeclarationCompletionSite {
    fn new(
        source: IndexedModuleSourceSite,
        has_path_attribute: bool,
        member_prefix_span: Span,
    ) -> Self {
        Self {
            source,
            has_path_attribute,
            member_prefix_span,
        }
    }

    pub(crate) fn source(&self) -> &IndexedModuleSourceSite {
        &self.source
    }

    pub(crate) fn has_path_attribute(&self) -> bool {
        self.has_path_attribute
    }

    pub(crate) fn replace_span(&self) -> Span {
        self.member_prefix_span
    }
}

/// Member-access site carrying the indexed receiver and the partial member replacement span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DotCompletionSite {
    source: IndexedMemberAccessSite,
}

impl DotCompletionSite {
    fn new(source: IndexedMemberAccessSite) -> Self {
        Self { source }
    }

    pub(crate) fn replace_span(&self) -> Span {
        self.source.member_prefix_span()
    }

    pub(crate) fn receiver_span(&self) -> Span {
        self.source.receiver_span()
    }

    pub(crate) fn source(&self) -> IndexedMemberAccessSite {
        self.source
    }
}

/// Qualified path site whose indexed scope can expose module or associated-item candidates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PathCompletionSite {
    source: IndexedQualifiedPathSite,
}

impl PathCompletionSite {
    fn new(source: IndexedQualifiedPathSite) -> Self {
        Self { source }
    }

    pub(crate) fn replace_span(&self) -> Span {
        self.source.member_prefix_span()
    }

    pub(crate) fn context(&self) -> NameCompletionContext {
        match self.source.scope() {
            IndexedQualifiedPathScope::Body { context, .. } => match context {
                IndexedQualifiedPathContext::Type => NameCompletionContext::Type,
                IndexedQualifiedPathContext::Value => NameCompletionContext::Value,
                IndexedQualifiedPathContext::Const => NameCompletionContext::Const,
                IndexedQualifiedPathContext::Pattern(kind) => {
                    NameCompletionContext::Pattern(PatternCompletionKind::from(kind))
                }
            },
            IndexedQualifiedPathScope::Signature { .. } => NameCompletionContext::Type,
            IndexedQualifiedPathScope::Import { .. } => NameCompletionContext::Import,
        }
    }

    pub(crate) fn source(&self) -> &IndexedQualifiedPathSite {
        &self.source
    }
}

/// Namespace policy shared by qualified and unqualified semantic names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameCompletionContext {
    /// A type annotation or type argument.
    Type,
    /// An ordinary expression or callable position.
    Value,
    /// A const expression, where runtime values are not valid.
    Const,
    /// A pattern position together with its accepted constructor shape.
    Pattern(PatternCompletionKind),
    /// A `use` tree, where names are imported rather than evaluated.
    Import,
}

/// Request-local qualifier retained when a trailing `::` has no lowered final segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QualifiedPathCompletionSyntax {
    qualifier: String,
    context: NameCompletionContext,
}

impl QualifiedPathCompletionSyntax {
    pub(crate) fn new(qualifier: String, context: NameCompletionContext) -> Self {
        Self { qualifier, context }
    }

    pub(crate) fn qualifier(&self) -> &str {
        &self.qualifier
    }

    pub(crate) fn context(&self) -> NameCompletionContext {
        self.context
    }
}

/// Pattern shape used by completion filters and renderers after source normalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PatternCompletionKind {
    /// A bare pattern name that may still grow into tuple or record constructor syntax.
    Name,
    /// A pattern already followed by tuple delimiters, such as `Some($0)`.
    TupleConstructor,
    /// A pattern already followed by record delimiters, such as `User { $0 }`.
    RecordConstructor,
}

impl From<IndexedPatternCompletionKind> for PatternCompletionKind {
    fn from(kind: IndexedPatternCompletionKind) -> Self {
        match kind {
            IndexedPatternCompletionKind::Name => Self::Name,
            IndexedPatternCompletionKind::TupleConstructor => Self::TupleConstructor,
            IndexedPatternCompletionKind::RecordConstructor => Self::RecordConstructor,
        }
    }
}

/// Name without a written `.` or `::`, together with its lexical/signature/import scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnqualifiedCompletionSite {
    source: IndexedUnqualifiedNameSite,
}

impl UnqualifiedCompletionSite {
    pub(crate) fn new(source: IndexedUnqualifiedNameSite) -> Self {
        Self { source }
    }

    pub(crate) fn replace_span(&self) -> Span {
        self.source.member_prefix_span()
    }

    pub(crate) fn member_prefix(&self) -> &str {
        self.source.member_prefix()
    }

    pub(crate) fn context(&self) -> NameCompletionContext {
        match self.source.scope() {
            IndexedUnqualifiedNameScope::Body { context, .. }
            | IndexedUnqualifiedNameScope::Signature { context, .. } => match context {
                IndexedUnqualifiedNameContext::Type { .. } => NameCompletionContext::Type,
                IndexedUnqualifiedNameContext::Value => NameCompletionContext::Value,
                IndexedUnqualifiedNameContext::Const => NameCompletionContext::Const,
                IndexedUnqualifiedNameContext::Pattern(kind) => {
                    NameCompletionContext::Pattern(PatternCompletionKind::from(*kind))
                }
            },
            IndexedUnqualifiedNameScope::Import { .. } => NameCompletionContext::Import,
        }
    }

    pub(crate) fn includes_keyword_overlay(&self) -> bool {
        matches!(
            self.context(),
            NameCompletionContext::Type
                | NameCompletionContext::Value
                | NameCompletionContext::Pattern(_)
        )
    }

    pub(crate) fn source(&self) -> &IndexedUnqualifiedNameSite {
        &self.source
    }
}

/// Named field position inside a resolved struct or enum-variant literal/pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordFieldCompletionSite {
    source: IndexedRecordFieldListSite,
}

/// Request-local syntax contexts that do not need a semantic source site of their own.
///
/// The syntax classifier is allowed to use parser nodes while discovering these contexts, but
/// candidate lookup consumes only this completion-domain vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SyntaxCompletionContext {
    EmptyPath(EmptyPathCompletionContext),
    Type(TypeCompletionContext),
    Pattern(PatternCompletionKind),
    ItemList(ItemListCompletionContext),
    BodyMacro(BodyMacroCompletionContext),
    ModuleMacro(ModuleMacroCompletionContext),
    ModuleDeclaration(ModuleDeclarationCompletionContext),
    Statement,
    Expression,
    Specialized(SpecializedCompletionContext),
}

/// Macro callee recognized from request syntax while its semantic scope still comes from a body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyMacroCompletionContext {
    qualifier: Option<Path>,
}

impl BodyMacroCompletionContext {
    pub(crate) fn new(qualifier: Option<Path>) -> Self {
        Self { qualifier }
    }

    pub(crate) fn qualifier(&self) -> Option<&Path> {
        self.qualifier.as_ref()
    }
}

/// Attribute grammar selected at the cursor, including entries already written beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttributeCompletionContext {
    kind: AttributeCompletionKind,
}

impl AttributeCompletionContext {
    pub(crate) fn new(kind: AttributeCompletionKind) -> Self {
        Self { kind }
    }

    pub(crate) fn kind(&self) -> &AttributeCompletionKind {
        &self.kind
    }
}

/// Attribute-owned candidate vocabulary selected from the surrounding attribute syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AttributeCompletionKind {
    /// Attribute name path, optionally below a resolved module qualifier.
    Path { qualifier: Option<Path> },
    /// `#[derive(...)]` input; `existing` prevents duplicate derives.
    Derive {
        qualifier: Option<Path>,
        existing: Vec<String>,
    },
    /// Lint-name input such as `#[allow(...)]`.
    Lint { existing: Vec<String> },
    /// Representation hint input inside `#[repr(...)]`.
    Repr { existing: Vec<String> },
    /// A predicate inside `#[cfg(...)]` or `#[cfg_attr(...)]`.
    Cfg,
    /// A Cargo feature string inside a `cfg(feature = "...")` predicate.
    CfgFeature { existing: Vec<String> },
    /// Arguments of `#[diagnostic::on_unimplemented(...)]`.
    Diagnostic { existing: Vec<String> },
    /// Stability or deprecation metadata such as `feature`, `since`, or `note`.
    Compatibility {
        attribute: String,
        existing: Vec<String>,
    },
}

/// Path and keyword capabilities inside a restricted visibility such as `pub(in crate::api)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestrictedVisibilityCompletionContext {
    qualifier: Option<Path>,
    allows_in_keyword: bool,
}

impl RestrictedVisibilityCompletionContext {
    pub(crate) fn new(qualifier: Option<Path>, allows_in_keyword: bool) -> Self {
        Self {
            qualifier,
            allows_in_keyword,
        }
    }

    pub(crate) fn qualifier(&self) -> Option<&Path> {
        self.qualifier.as_ref()
    }

    pub(crate) fn allows_in_keyword(&self) -> bool {
        self.allows_in_keyword
    }
}

/// Optional qualifier for a value path known to occur in a compile-time const expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConstExpressionCompletionContext {
    qualifier: Option<Path>,
}

impl ConstExpressionCompletionContext {
    pub(crate) fn new(qualifier: Option<Path>) -> Self {
        Self { qualifier }
    }

    pub(crate) fn qualifier(&self) -> Option<&Path> {
        self.qualifier.as_ref()
    }
}

/// Lifetime use/declaration site plus higher-ranked lifetimes visible only in request syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LifetimeCompletionContext {
    declaration: bool,
    binder_lifetimes: Vec<String>,
}

impl LifetimeCompletionContext {
    pub(crate) fn new(declaration: bool, binder_lifetimes: Vec<String>) -> Self {
        Self {
            declaration,
            binder_lifetimes,
        }
    }

    pub(crate) fn is_declaration(&self) -> bool {
        self.declaration
    }

    pub(crate) fn binder_lifetimes(&self) -> &[String] {
        &self.binder_lifetimes
    }
}

/// Loop-label use or declaration selected before body-scope label lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LabelCompletionContext {
    declaration: bool,
}

impl LabelCompletionContext {
    pub(crate) fn new(declaration: bool) -> Self {
        Self { declaration }
    }

    pub(crate) fn is_declaration(self) -> bool {
        self.declaration
    }
}

/// String literal whose contents belong to a Rust-owned completion grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SpecializedStringCompletionContext {
    /// A format string, including explicit named arguments that shadow captured locals.
    Format { named_arguments: Vec<String> },
    /// A compile-time environment variable name accepted by `env!` or `option_env!`.
    Environment,
    /// An ABI name inside an `extern "..."` declaration.
    Abi,
}

/// Item-position macro syntax, optionally retaining item keywords for an incomplete callee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModuleMacroCompletionContext {
    qualifier: Option<Path>,
    incomplete_item_list: Option<ItemListCompletionContext>,
}

impl ModuleMacroCompletionContext {
    pub(crate) fn new(qualifier: Option<Path>) -> Self {
        Self {
            qualifier,
            incomplete_item_list: None,
        }
    }

    pub(crate) fn incomplete(
        qualifier: Option<Path>,
        item_list: ItemListCompletionContext,
    ) -> Self {
        Self {
            qualifier,
            incomplete_item_list: Some(item_list),
        }
    }

    pub(crate) fn qualifier(&self) -> Option<&Path> {
        self.qualifier.as_ref()
    }

    pub(crate) fn incomplete_item_list(&self) -> Option<ItemListCompletionContext> {
        self.incomplete_item_list
    }
}

/// Out-of-line `mod name;` syntax and whether `#[path = ...]` overrides normal file discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ModuleDeclarationCompletionContext {
    has_path_attribute: bool,
}

impl ModuleDeclarationCompletionContext {
    pub(crate) fn new(has_path_attribute: bool) -> Self {
        Self { has_path_attribute }
    }

    pub(crate) fn has_path_attribute(self) -> bool {
        self.has_path_attribute
    }
}

/// Explicit empty path positions recognized from request-local syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmptyPathCompletionContext {
    Import,
    Type,
    Expression,
    Argument,
    GenericArgument,
}

/// Type-position capabilities that affect keyword validity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypeCompletionContext {
    General,
    ImplTraitAllowed,
}

impl TypeCompletionContext {
    pub(crate) fn allows_impl_trait(self) -> bool {
        matches!(self, Self::ImplTraitAllowed)
    }
}

/// The syntactic owner of an item list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ItemListCompletionKind {
    SourceFile,
    Module,
    InherentImpl,
    Trait,
    TraitImpl,
    ExternBlock { is_unsafe: bool },
}

/// Item-list context after normalizing both ownership and already-written qualifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ItemListCompletionContext {
    kind: ItemListCompletionKind,
    qualifiers: ItemQualifierContext,
}

impl ItemListCompletionContext {
    pub(crate) fn new(kind: ItemListCompletionKind, qualifiers: ItemQualifierContext) -> Self {
        Self { kind, qualifiers }
    }

    pub(crate) fn kind(self) -> ItemListCompletionKind {
        self.kind
    }

    pub(crate) fn qualifiers(self) -> ItemQualifierContext {
        self.qualifiers
    }
}

/// Qualifiers already written before an incomplete item keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ItemQualifierContext {
    pub(crate) has_visibility: bool,
    pub(crate) has_unsafe: bool,
    pub(crate) has_async: bool,
    pub(crate) has_extern: bool,
    pub(crate) has_const: bool,
}

/// Narrow syntax families that must suppress the generic item keyword fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SpecializedCompletionContext {
    Attribute(AttributeCompletionContext),
    ConstExpression(ConstExpressionCompletionContext),
    ExternCrateName,
    Label(LabelCompletionContext),
    Lifetime(LifetimeCompletionContext),
    MacroFragment,
    RestrictedVisibility(RestrictedVisibilityCompletionContext),
    String(SpecializedStringCompletionContext),
}

impl RecordFieldCompletionSite {
    fn new(source: IndexedRecordFieldListSite) -> Self {
        Self { source }
    }

    pub(crate) fn replace_span(&self) -> Span {
        self.source.member_prefix_span()
    }

    pub(crate) fn source(&self) -> &IndexedRecordFieldListSite {
        &self.source
    }
}

/// Selects one completion family without leaking source-scanner types into query assembly.
///
/// Decisive parser hints handle `use`, `.`, and `::` cheaply. In incomplete syntax where those
/// hints are unavailable, the detector asks domain scanners from the most specific body-owned
/// shape through signature and import fallbacks.
pub(crate) struct CompletionSiteDetector<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> CompletionSiteDetector<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    /// Classifies the cursor offset by asking the scanner that owns each syntax shape.
    pub(crate) fn site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
        syntax: Option<CompletionSiteSyntax>,
    ) -> anyhow::Result<Option<CompletionSite>> {
        let source = SourceCompletionView::new(self.db);
        if let Some(syntax) = syntax {
            if let Some(standalone) = syntax.standalone {
                let Some(module_or_impl_site) = (match standalone {
                    StandaloneCompletionSiteSyntax::TraitImpl(syntax) => {
                        let TraitImplCompletionSyntax {
                            owner_start,
                            member_kind,
                            replace_span,
                            lookup_prefix,
                        } = syntax;
                        return Ok(source
                            .trait_impl_site_at(crate_ref, file_id, owner_start)
                            .context("scan trait impl completion site")?
                            .map(|source| {
                                CompletionSite::TraitImpl(TraitImplCompletionSite::new(
                                    source,
                                    member_kind,
                                    replace_span,
                                    lookup_prefix,
                                ))
                            }));
                    }
                    StandaloneCompletionSiteSyntax::BodyMacro { qualifier } => {
                        if let Some(qualifier) = qualifier {
                            return Ok(source
                                .body_syntax_qualified_path_site_at(
                                    crate_ref,
                                    file_id,
                                    offset,
                                    qualifier,
                                    syntax.member_prefix_span,
                                    syntax.member_prefix,
                                )
                                .context("scan qualified body macro callee site")?
                                .map(PathCompletionSite::new)
                                .map(CompletionSite::Path));
                        }
                        return Ok(source
                            .body_syntax_name_site_at(
                                crate_ref,
                                file_id,
                                offset,
                                IndexedUnqualifiedNameContext::Value,
                                syntax.member_prefix_span,
                                syntax.member_prefix,
                            )
                            .context("scan unqualified body macro callee site")?
                            .map(UnqualifiedCompletionSite::new)
                            .map(CompletionSite::Unqualified));
                    }
                    StandaloneCompletionSiteSyntax::ModuleMacro { qualifier } => source
                        .module_source_site_at(crate_ref, file_id, offset)
                        .context("scan module macro completion site")?
                        .map(|source| {
                            CompletionSite::ModuleMacro(ModuleMacroCompletionSite::new(
                                source,
                                qualifier,
                                syntax.member_prefix_span,
                            ))
                        }),
                    StandaloneCompletionSiteSyntax::ModuleDeclaration { has_path_attribute } => {
                        source
                            .module_source_site_at(crate_ref, file_id, offset)
                            .context("scan module declaration completion site")?
                            .map(|source| {
                                CompletionSite::ModuleDeclaration(
                                    ModuleDeclarationCompletionSite::new(
                                        source,
                                        has_path_attribute,
                                        syntax.member_prefix_span,
                                    ),
                                )
                            })
                    }
                }) else {
                    return Ok(None);
                };
                return Ok(Some(module_or_impl_site));
            }
            if let Some(owner) = syntax.empty_record_owner {
                let site = source
                    .record_syntax_field_list_site_at(
                        crate_ref,
                        file_id,
                        offset,
                        owner,
                        syntax.member_prefix_span,
                        syntax.body_owner_start,
                    )
                    .context("scan syntax-owned empty record field site")?;
                return Ok(site
                    .map(RecordFieldCompletionSite::new)
                    .map(CompletionSite::RecordField));
            }
            if let Some(empty_path) = syntax.empty_path {
                let indexed_context = match empty_path {
                    EmptyPathCompletionContext::Type => IndexedUnqualifiedNameContext::Type {
                        position: IndexedTypeNamePosition::Type,
                    },
                    EmptyPathCompletionContext::GenericArgument => {
                        IndexedUnqualifiedNameContext::Type {
                            position: IndexedTypeNamePosition::BareGenericArgument,
                        }
                    }
                    EmptyPathCompletionContext::Expression
                    | EmptyPathCompletionContext::Argument => IndexedUnqualifiedNameContext::Value,
                    EmptyPathCompletionContext::Import => {
                        return Ok(source
                            .import_empty_name_site_at(crate_ref, file_id, offset)
                            .context("scan empty import completion site")?
                            .map(UnqualifiedCompletionSite::new)
                            .map(CompletionSite::Unqualified));
                    }
                };
                if let Some(site) = source
                    .body_empty_name_site_at(crate_ref, file_id, offset, indexed_context)
                    .context("scan empty body completion site")?
                {
                    return Ok(Some(CompletionSite::Unqualified(
                        UnqualifiedCompletionSite::new(site),
                    )));
                }
                if let IndexedUnqualifiedNameContext::Type { position } = indexed_context {
                    return Ok(source
                        .signature_empty_type_site_at(crate_ref, file_id, offset, position)
                        .context("scan empty signature completion site")?
                        .map(UnqualifiedCompletionSite::new)
                        .map(CompletionSite::Unqualified));
                }
                return Ok(None);
            }
            if syntax.inside_use_item {
                if let Some(site) = source
                    .import_qualified_path_site_at(crate_ref, file_id, offset)
                    .context("scan qualified import completion site")?
                {
                    return Ok(Some(CompletionSite::Path(PathCompletionSite::new(site))));
                }

                return Ok(source
                    .import_unqualified_name_site_at(crate_ref, file_id, offset)
                    .context("scan unqualified import completion site")?
                    .map(UnqualifiedCompletionSite::new)
                    .map(CompletionSite::Unqualified));
            }
            if syntax.after_dot {
                return Ok(source
                    .member_access_site_at(crate_ref, file_id, offset)
                    .context("scan member access completion site")?
                    .map(DotCompletionSite::new)
                    .map(CompletionSite::Dot));
            }
            if syntax.after_colon_colon {
                if let Some(path) = syntax.empty_qualified_path {
                    if let Some(site) = source
                        .body_syntax_rich_qualified_path_site_at(
                            crate_ref,
                            file_id,
                            offset,
                            path.qualifier(),
                            Self::indexed_qualified_path_context(path.context()),
                            syntax.member_prefix_span,
                            syntax.body_owner_start,
                        )
                        .context("scan request-local empty qualified body path")?
                    {
                        return Ok(Some(CompletionSite::Path(PathCompletionSite::new(site))));
                    }

                    if matches!(path.context(), NameCompletionContext::Type)
                        && let Some(site) = source
                            .signature_syntax_rich_qualified_path_site_at(
                                crate_ref,
                                file_id,
                                offset,
                                path.qualifier(),
                                syntax.member_prefix_span,
                            )
                            .context("scan request-local empty qualified signature path")?
                    {
                        return Ok(Some(CompletionSite::Path(PathCompletionSite::new(site))));
                    }
                    return Ok(None);
                }

                if let Some(site) = source
                    .body_qualified_path_site_at(crate_ref, file_id, offset)
                    .context("scan body qualified completion site")?
                {
                    return Ok(Some(CompletionSite::Path(PathCompletionSite::new(site))));
                }

                return Ok(source
                    .signature_type_site_at(crate_ref, file_id, offset)
                    .context("scan qualified signature completion site")?
                    .map(CompletionSite::from_signature_type_site));
            }
        }

        // Without a decisive syntax hint, ask scanners in the order that preserves the most
        // specific source interpretation: member access, qualified path, record field, lexical
        // body name, declaration signature, then import path fallback. Most requests happen in a
        // body, so signature scanning stays behind the body-owned sites.
        if let Some(site) = source
            .member_access_site_at(crate_ref, file_id, offset)
            .context("scan member access completion site")?
        {
            return Ok(Some(CompletionSite::Dot(DotCompletionSite::new(site))));
        }

        if let Some(site) = source
            .body_associated_type_binding_site_at(crate_ref, file_id, offset)
            .context("scan body associated type binding completion site")?
        {
            return Ok(Some(CompletionSite::AssociatedTypeBinding(site)));
        }

        if let Some(site) = source
            .body_qualified_path_site_at(crate_ref, file_id, offset)
            .context("scan body qualified completion site")?
        {
            return Ok(Some(CompletionSite::Path(PathCompletionSite::new(site))));
        }

        if let Some(site) = source
            .record_field_list_site_at(crate_ref, file_id, offset)
            .context("scan record field completion site")?
        {
            return Ok(Some(CompletionSite::RecordField(
                RecordFieldCompletionSite::new(site),
            )));
        }

        if let Some(site) = source
            .body_unqualified_name_site_at(crate_ref, file_id, offset)
            .context("scan unqualified body completion site")?
        {
            return Ok(Some(CompletionSite::Unqualified(
                UnqualifiedCompletionSite::new(site),
            )));
        }

        if let Some(site) = source
            .signature_type_site_at(crate_ref, file_id, offset)
            .context("scan signature completion site")?
        {
            return Ok(Some(CompletionSite::from_signature_type_site(site)));
        }

        if let Some(site) = source
            .import_qualified_path_site_at(crate_ref, file_id, offset)
            .context("scan qualified import completion site")?
        {
            return Ok(Some(CompletionSite::Path(PathCompletionSite::new(site))));
        }

        Ok(source
            .import_unqualified_name_site_at(crate_ref, file_id, offset)
            .context("scan unqualified import completion site")?
            .map(UnqualifiedCompletionSite::new)
            .map(CompletionSite::Unqualified))
    }

    fn indexed_qualified_path_context(
        context: NameCompletionContext,
    ) -> IndexedQualifiedPathContext {
        match context {
            NameCompletionContext::Type => IndexedQualifiedPathContext::Type,
            NameCompletionContext::Value => IndexedQualifiedPathContext::Value,
            NameCompletionContext::Const => IndexedQualifiedPathContext::Const,
            NameCompletionContext::Pattern(kind) => {
                IndexedQualifiedPathContext::Pattern(match kind {
                    PatternCompletionKind::Name => IndexedPatternCompletionKind::Name,
                    PatternCompletionKind::TupleConstructor => {
                        IndexedPatternCompletionKind::TupleConstructor
                    }
                    PatternCompletionKind::RecordConstructor => {
                        IndexedPatternCompletionKind::RecordConstructor
                    }
                })
            }
            NameCompletionContext::Import => {
                unreachable!("request-local body qualifiers cannot belong to imports")
            }
        }
    }
}
