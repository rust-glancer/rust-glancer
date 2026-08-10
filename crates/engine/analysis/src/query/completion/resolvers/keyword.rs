//! Conservative keyword and small-snippet completion assembly.
//!
//! Incomplete source like `f$0` or `ma$0` often cannot lower into a semantic cursor site, but the
//! speculative syntax context can still distinguish item lists, statements, expressions, types,
//! and patterns. Each context has a deliberately small vocabulary. Item-list candidates also
//! account for the owner (`mod`, trait, impl, or extern block) and qualifiers already written, so
//! accepting a row does not duplicate syntax such as `pub`, `unsafe`, `async`, or `extern`.
//!
//! Semantic name completion may request a lower-priority keyword overlay. This keeps constructs
//! such as `return`, `dyn`, or `mut` available without letting them outrank declarations found in
//! the actual scope.

use crate::model::{
    CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem, CompletionKind,
    CompletionTarget, KeywordCompletion,
};
use crate::query::completion::site::{
    ItemListCompletionContext, ItemListCompletionKind, SyntaxCompletionContext,
    TypeCompletionContext,
};

use super::super::{CompletionClientCapabilities, syntax::CompletionSyntaxContext};

/// Builds the language-owned rows that can be selected from request syntax alone.
///
/// It is also used as an overlay after semantic lookup. In that mode the same keyword candidates
/// receive a later sort bucket, so declarations remain the first results.
pub(super) struct KeywordCompletionResolver {
    client_capabilities: CompletionClientCapabilities,
}

impl KeywordCompletionResolver {
    pub(super) fn new(client_capabilities: CompletionClientCapabilities) -> Self {
        Self {
            client_capabilities,
        }
    }

    /// Collects keyword completions for plain source positions like `ma$0` or `fn $0`.
    pub(super) fn completions(
        &self,
        syntax: Option<&CompletionSyntaxContext<'_>>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        self.completions_at_with_sort(syntax, KeywordSortPosition::Primary)
    }

    /// Keeps item keywords available while a bare identifier is also treated as a possible
    /// module-scope macro invocation before its `!` has been typed.
    pub(super) fn item_list_completions(
        &self,
        context: ItemListCompletionContext,
        syntax: Option<&CompletionSyntaxContext<'_>>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let Some(syntax) = syntax else {
            return Ok(Vec::new());
        };
        Ok(self.completion_items(
            &KeywordCandidate::for_item_list(context),
            syntax,
            KeywordSortPosition::Primary,
        ))
    }

    /// Collects lower-priority keyword rows to append after semantic name completions.
    pub(super) fn overlay_completions(
        &self,
        syntax: Option<&CompletionSyntaxContext<'_>>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        self.completions_at_with_sort(syntax, KeywordSortPosition::Overlay)
    }

    /// Collects keywords that are valid at a semantic pattern site.
    ///
    /// The body scanner has already established that the cursor belongs to a pattern, so this
    /// path intentionally does not reuse the coarser item/statement/expression classifier.
    pub(super) fn pattern_overlay_completions(
        &self,
        syntax: Option<&CompletionSyntaxContext<'_>>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let Some(syntax) = syntax else {
            return Ok(Vec::new());
        };
        Ok(self.completion_items(
            KeywordCandidate::PATTERN,
            syntax,
            KeywordSortPosition::Overlay,
        ))
    }

    /// Collects type keywords while using the semantic site as proof that this is a type position.
    pub(super) fn type_overlay_completions(
        &self,
        syntax: Option<&CompletionSyntaxContext<'_>>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let Some(syntax) = syntax else {
            return Ok(Vec::new());
        };
        let context = match syntax.completion_context() {
            Some(SyntaxCompletionContext::Type(context)) => context,
            _ => TypeCompletionContext::General,
        };
        let candidates = KeywordCandidate::for_type(context);
        Ok(self.completion_items(&candidates, syntax, KeywordSortPosition::Overlay))
    }

    /// Collects roots that can begin an ordinary Rust path at a semantic name site.
    ///
    /// These rows are separate from expression/type keywords: `crate`, `self`, and `super` are
    /// valid in imports too, and their meaning here is specifically to start module resolution.
    pub(super) fn path_root_overlay_completions(
        &self,
        prefix: &str,
        edit: CompletionEdit,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let mut completions = KeywordCandidate::PATH_ROOTS
            .iter()
            .filter(|candidate| candidate.label.starts_with(prefix))
            .map(|candidate| {
                candidate.completion_item(
                    edit,
                    KeywordSortPosition::Overlay,
                    self.client_capabilities,
                )
            })
            .collect::<Vec<_>>();
        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }

    fn completions_at_with_sort(
        &self,
        syntax: Option<&CompletionSyntaxContext<'_>>,
        sort: KeywordSortPosition,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        // First identify the raw source prefix and the coarse syntactic region
        // around it; candidate selection is intentionally small and context-led.
        let Some(syntax) = syntax else {
            return Ok(Vec::new());
        };
        let Some(context) = syntax.completion_context() else {
            return Ok(Vec::new());
        };
        let candidates = KeywordCandidate::for_context(context);
        Ok(self.completion_items(&candidates, syntax, sort))
    }

    fn completion_items(
        &self,
        candidates: &[KeywordCandidate],
        syntax: &CompletionSyntaxContext<'_>,
        sort: KeywordSortPosition,
    ) -> Vec<CompletionItem> {
        let prefix = syntax.prefix();
        let edit = CompletionEdit {
            replace: prefix.span(),
        };
        let mut completions = candidates
            .iter()
            .filter(|candidate| candidate.label.starts_with(prefix.text()))
            .map(|candidate| candidate.completion_item(edit, sort, self.client_capabilities))
            .collect::<Vec<_>>();

        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        completions
    }
}

/// Chooses whether keyword rows stand on their own or sit behind semantic names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeywordSortPosition {
    Primary,
    Overlay,
}

impl KeywordSortPosition {
    /// Builds a sort bucket for standalone keyword rows or lower-priority overlays.
    fn sort_text(self, rank: u8, label: &str) -> String {
        match self {
            Self::Primary => format!("00-keyword:{rank:02}:{label}"),
            Self::Overlay => format!("~keyword:{rank:02}:{label}"),
        }
    }
}

/// One keyword row and its optional snippet body.
#[derive(Debug, Clone, Copy)]
struct KeywordCandidate {
    keyword: KeywordCompletion,
    label: &'static str,
    snippet: Option<&'static str>,
    sort_rank: u8,
}

impl KeywordCandidate {
    const PATH_ROOTS: &'static [Self] = &[
        Self::new(KeywordCompletion::Crate, "crate", Some("crate::$0"), 0),
        Self::new(KeywordCompletion::SelfValue, "self", Some("self::$0"), 1),
        Self::new(KeywordCompletion::Super, "super", Some("super::$0"), 2),
    ];

    const STATEMENT: &'static [Self] = &[
        Self::new(
            KeywordCompletion::Let,
            "let",
            Some("let ${1:name} = $0;"),
            0,
        ),
        Self::new(KeywordCompletion::Return, "return", None, 1),
        Self::new(
            KeywordCompletion::If,
            "if",
            Some("if ${1:condition} {\n    $0\n}"),
            2,
        ),
        Self::new(
            KeywordCompletion::Match,
            "match",
            Some("match ${1:value} {\n    $0\n}"),
            3,
        ),
        Self::new(KeywordCompletion::While, "while", None, 4),
        Self::new(
            KeywordCompletion::Loop,
            "loop",
            Some("loop {\n    $0\n}"),
            5,
        ),
        Self::new(KeywordCompletion::For, "for", None, 6),
    ];

    const EXPRESSION: &'static [Self] = &[
        Self::new(
            KeywordCompletion::If,
            "if",
            Some("if ${1:condition} {\n    $0\n}"),
            0,
        ),
        Self::new(
            KeywordCompletion::Match,
            "match",
            Some("match ${1:value} {\n    $0\n}"),
            1,
        ),
        Self::new(
            KeywordCompletion::Loop,
            "loop",
            Some("loop {\n    $0\n}"),
            2,
        ),
        Self::new(KeywordCompletion::Return, "return", None, 3),
        Self::new(KeywordCompletion::True, "true", None, 4),
        Self::new(KeywordCompletion::False, "false", None, 5),
        Self::new(KeywordCompletion::Async, "async", None, 6),
        Self::new(KeywordCompletion::Move, "move", None, 7),
    ];

    const PATTERN: &'static [Self] = &[
        Self::new(KeywordCompletion::Ref, "ref", None, 0),
        Self::new(KeywordCompletion::Mut, "mut", None, 1),
        Self::new(KeywordCompletion::True, "true", None, 2),
        Self::new(KeywordCompletion::False, "false", None, 3),
    ];

    const PUB: Self = Self::new(KeywordCompletion::Pub, "pub", Some("pub $0"), 0);
    const UNSAFE: Self = Self::new(KeywordCompletion::Unsafe, "unsafe", Some("unsafe $0"), 1);
    const ASYNC: Self = Self::new(KeywordCompletion::Async, "async", Some("async $0"), 2);
    const EXTERN: Self = Self::new(KeywordCompletion::Extern, "extern", Some("extern $0"), 3);
    const FN_BODY: Self = Self::new(
        KeywordCompletion::Fn,
        "fn",
        Some("fn ${1:name}(${2:args}) {\n    $0\n}"),
        4,
    );
    const FN_DECL: Self = Self::new(
        KeywordCompletion::Fn,
        "fn",
        Some("fn ${1:name}(${2:args});"),
        4,
    );
    const CONST_VALUE: Self = Self::new(
        KeywordCompletion::Const,
        "const",
        Some("const ${1:NAME}: ${2:Type} = $0;"),
        5,
    );
    const CONST_DECL: Self = Self::new(
        KeywordCompletion::Const,
        "const",
        Some("const ${1:NAME}: ${2:Type};"),
        5,
    );
    const TYPE_VALUE: Self = Self::new(
        KeywordCompletion::Type,
        "type",
        Some("type ${1:Name} = ${2:Type};"),
        6,
    );
    const TYPE_DECL: Self = Self::new(KeywordCompletion::Type, "type", Some("type ${1:Name};"), 6);
    const STATIC_VALUE: Self = Self::new(
        KeywordCompletion::Static,
        "static",
        Some("static ${1:NAME}: ${2:Type} = $0;"),
        7,
    );
    const STATIC_DECL: Self = Self::new(
        KeywordCompletion::Static,
        "static",
        Some("static ${1:NAME}: ${2:Type};"),
        7,
    );
    const STRUCT: Self = Self::new(
        KeywordCompletion::Struct,
        "struct",
        Some("struct ${1:Name} {\n    $0\n}"),
        8,
    );
    const ENUM: Self = Self::new(
        KeywordCompletion::Enum,
        "enum",
        Some("enum ${1:Name} {\n    $0\n}"),
        9,
    );
    const TRAIT: Self = Self::new(
        KeywordCompletion::Trait,
        "trait",
        Some("trait ${1:Name} {\n    $0\n}"),
        10,
    );
    const UNION: Self = Self::new(
        KeywordCompletion::Union,
        "union",
        Some("union ${1:Name} {\n    $0\n}"),
        11,
    );
    const IMPL: Self = Self::new(
        KeywordCompletion::Impl,
        "impl",
        Some("impl ${1:Type} {\n    $0\n}"),
        12,
    );
    const IMPL_FOR: Self = Self::new(
        KeywordCompletion::ImplFor,
        "impl for",
        Some("impl ${1:Trait} for ${2:Type} {\n    $0\n}"),
        13,
    );
    const MOD: Self = Self::new(KeywordCompletion::Mod, "mod", Some("mod ${1:name};"), 14);
    const USE: Self = Self::new(KeywordCompletion::Use, "use", Some("use ${1:path};"), 15);
    const CRATE: Self = Self::new(
        KeywordCompletion::Crate,
        "crate",
        Some("crate ${1:name};"),
        16,
    );

    const fn new(
        keyword: KeywordCompletion,
        label: &'static str,
        snippet: Option<&'static str>,
        sort_rank: u8,
    ) -> Self {
        Self {
            keyword,
            label,
            snippet,
            sort_rank,
        }
    }

    fn for_context(context: SyntaxCompletionContext) -> Vec<Self> {
        match context {
            SyntaxCompletionContext::EmptyPath(context) => match context {
                crate::query::completion::site::EmptyPathCompletionContext::Type
                | crate::query::completion::site::EmptyPathCompletionContext::GenericArgument => {
                    Self::for_type(TypeCompletionContext::General)
                }
                crate::query::completion::site::EmptyPathCompletionContext::Expression
                | crate::query::completion::site::EmptyPathCompletionContext::Argument => {
                    Self::EXPRESSION.to_vec()
                }
                crate::query::completion::site::EmptyPathCompletionContext::Import => Vec::new(),
            },
            SyntaxCompletionContext::Type(context) => Self::for_type(context),
            SyntaxCompletionContext::Pattern(_) => Self::PATTERN.to_vec(),
            SyntaxCompletionContext::ItemList(context) => Self::for_item_list(context),
            SyntaxCompletionContext::BodyMacro(_)
            | SyntaxCompletionContext::ModuleMacro(_)
            | SyntaxCompletionContext::ModuleDeclaration(_) => Vec::new(),
            SyntaxCompletionContext::Statement => Self::STATEMENT.to_vec(),
            SyntaxCompletionContext::Expression => Self::EXPRESSION.to_vec(),
            SyntaxCompletionContext::Specialized(_) => Vec::new(),
        }
    }

    fn for_type(context: TypeCompletionContext) -> Vec<Self> {
        let mut candidates = vec![
            Self::new(KeywordCompletion::Dyn, "dyn", Some("dyn ${1:Trait}"), 0),
            Self::new(
                KeywordCompletion::Fn,
                "fn",
                Some("fn(${1:Args}) -> ${2:Return}"),
                1,
            ),
            Self::new(
                KeywordCompletion::For,
                "for",
                Some("for<'${1:a}> ${2:Type}"),
                2,
            ),
        ];
        if context.allows_impl_trait() {
            candidates.push(Self::new(
                KeywordCompletion::Impl,
                "impl",
                Some("impl ${1:Trait}"),
                3,
            ));
        }
        candidates
    }

    /// Selects only keywords that can continue this item-list owner and qualifier sequence.
    fn for_item_list(context: ItemListCompletionContext) -> Vec<Self> {
        let kind = context.kind();
        let qualifiers = context.qualifiers();
        let fn_candidate = match kind {
            ItemListCompletionKind::Trait | ItemListCompletionKind::ExternBlock { .. } => {
                Self::FN_DECL
            }
            ItemListCompletionKind::SourceFile
            | ItemListCompletionKind::Module
            | ItemListCompletionKind::InherentImpl
            | ItemListCompletionKind::TraitImpl => Self::FN_BODY,
        };

        if qualifiers.has_const {
            return match kind {
                ItemListCompletionKind::SourceFile
                | ItemListCompletionKind::Module
                | ItemListCompletionKind::InherentImpl => vec![fn_candidate],
                ItemListCompletionKind::Trait
                | ItemListCompletionKind::TraitImpl
                | ItemListCompletionKind::ExternBlock { .. } => Vec::new(),
            };
        }

        if qualifiers.has_extern {
            let mut candidates = vec![fn_candidate];
            if matches!(
                kind,
                ItemListCompletionKind::SourceFile | ItemListCompletionKind::Module
            ) && !qualifiers.has_async
                && !qualifiers.has_unsafe
            {
                candidates.push(Self::CRATE);
            }
            return candidates;
        }

        if qualifiers.has_async || qualifiers.has_unsafe {
            if matches!(kind, ItemListCompletionKind::ExternBlock { .. }) {
                return qualifiers
                    .has_unsafe
                    .then_some(fn_candidate)
                    .into_iter()
                    .collect();
            }

            let mut candidates = vec![fn_candidate];
            if !qualifiers.has_async {
                candidates.push(Self::ASYNC);
            }
            if !qualifiers.has_unsafe {
                candidates.push(Self::UNSAFE);
            }
            if qualifiers.has_unsafe
                && matches!(
                    kind,
                    ItemListCompletionKind::SourceFile | ItemListCompletionKind::Module
                )
            {
                candidates.push(Self::TRAIT);
                if !qualifiers.has_visibility {
                    candidates.extend([Self::IMPL, Self::IMPL_FOR]);
                }
                candidates.push(Self::EXTERN);
            }
            return candidates;
        }

        let mut candidates = Vec::new();
        if !qualifiers.has_visibility
            && !matches!(
                kind,
                ItemListCompletionKind::Trait | ItemListCompletionKind::TraitImpl
            )
        {
            candidates.push(Self::PUB);
        }

        match kind {
            ItemListCompletionKind::SourceFile | ItemListCompletionKind::Module => {
                candidates.extend([
                    Self::UNSAFE,
                    Self::ASYNC,
                    Self::EXTERN,
                    fn_candidate,
                    Self::CONST_VALUE,
                    Self::TYPE_VALUE,
                    Self::STATIC_VALUE,
                    Self::STRUCT,
                    Self::ENUM,
                    Self::TRAIT,
                    Self::UNION,
                    Self::MOD,
                    Self::USE,
                ]);
                if !qualifiers.has_visibility {
                    candidates.extend([Self::IMPL, Self::IMPL_FOR]);
                }
            }
            ItemListCompletionKind::InherentImpl => candidates.extend([
                Self::UNSAFE,
                Self::ASYNC,
                Self::EXTERN,
                fn_candidate,
                Self::CONST_VALUE,
            ]),
            ItemListCompletionKind::Trait => candidates.extend([
                Self::UNSAFE,
                Self::ASYNC,
                Self::EXTERN,
                fn_candidate,
                Self::CONST_DECL,
                Self::TYPE_DECL,
            ]),
            ItemListCompletionKind::TraitImpl => candidates.extend([
                Self::UNSAFE,
                Self::ASYNC,
                Self::EXTERN,
                fn_candidate,
                Self::CONST_VALUE,
                Self::TYPE_VALUE,
            ]),
            ItemListCompletionKind::ExternBlock { .. } => {
                candidates.extend([Self::UNSAFE, fn_candidate, Self::STATIC_DECL]);
            }
        }
        candidates
    }

    fn completion_item(
        self,
        edit: CompletionEdit,
        sort: KeywordSortPosition,
        client_capabilities: CompletionClientCapabilities,
    ) -> CompletionItem {
        let target = CompletionTarget::Keyword(self.keyword);
        CompletionItem {
            label: self.label.to_string(),
            filter_text: None,
            kind: CompletionKind::Keyword,
            target,
            applicability: CompletionApplicability::Known,
            detail: Some(format!("keyword {}", self.label)),
            documentation: None,
            sort_text: sort.sort_text(self.sort_rank, self.label),
            insert_text: if client_capabilities.snippet_support {
                self.snippet
                    .map(|snippet| CompletionInsertText::Snippet(snippet.to_string()))
                    .unwrap_or(CompletionInsertText::Plain)
            } else {
                CompletionInsertText::Plain
            },
            edit: Some(edit),
            additional_edits: Vec::new(),
        }
    }
}
