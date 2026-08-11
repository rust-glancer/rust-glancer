//! Request-local parsing and syntax classification for incomplete completion source.
//!
//! The saved syntax tree may not contain a node for what the user is still typing. This module
//! replaces the partial identifier with a known marker, reparses the request buffer, and recovers
//! the typed prefix, replacement span, nearby punctuation, and enclosing grammar.
//!
//! Focused child classifiers recognize attributes, apostrophe forms, strings and macro tokens,
//! paths, and item/signature positions. They return the completion-domain contexts from
//! `site`; semantic resolvers never need to inspect parser nodes. Incomplete forms are recovered
//! from that one tree's error nodes and neighboring tokens instead of by parsing punctuation-added
//! variants of the whole file.

mod apostrophe;
mod attribute;
mod item;
mod path;
mod string;

use std::sync::OnceLock;

use rg_parse::{Span, TextSpan};
use rg_syntax::{
    AstNode as _, AstToken as _, Edition, SourceFile, SyntaxKind, SyntaxToken, TextRange, TextSize,
    ast,
};

use crate::query::completion::site::{
    CompletionSiteSyntax, ItemListCompletionKind, PatternCompletionKind,
    SpecializedCompletionContext, StandaloneCompletionSiteSyntax, SyntaxCompletionContext,
};

/// Speculatively parsed request buffer centered on one completion offset.
///
/// The marker token makes incomplete text traversable with ordinary syntax ancestry. Construction
/// performs the request's only full-source parse and immediately stores the normalized completion
/// domain. Resolvers can inspect the same tree for small source-shape facts, but cannot trigger a
/// second parse by asking for classification again.
pub(super) struct CompletionSyntaxContext<'source> {
    source: &'source str,
    offset: u32,
    prefix: CompletionPrefix<'source>,
    file: SourceFile,
    marker: SyntaxToken,
    string_marker: Option<SyntaxToken>,
    source_map: CompletionSourceMap,
    classification: Option<SyntaxCompletionContext>,
    site_syntax: OnceLock<CompletionSiteSyntax>,
}

impl<'source> CompletionSyntaxContext<'source> {
    const MARKER: &'static str = "__rg_completion";

    /// Builds syntax context from saved source or a request-local editor buffer.
    pub(super) fn at(source_text: Option<&'source str>, offset: u32) -> Option<Self> {
        Self::from_source(source_text?, offset)
    }

    fn from_source(source: &'source str, offset: u32) -> Option<Self> {
        let cursor = usize::try_from(offset).ok()?;
        if cursor > source.len() || !source.is_char_boundary(cursor) {
            return None;
        }

        let prefix_start = Self::prefix_start(source, cursor);
        let prefix = CompletionPrefix {
            text: source.get(prefix_start..cursor)?,
            span: Span {
                text: TextSpan {
                    start: u32::try_from(prefix_start).ok()?,
                    end: offset,
                },
            },
        };

        let (file, marker, source_map) = Self::speculative_file(source, prefix_start, cursor)?;
        let string_marker = Self::string_marker(&marker);
        let mut context = Self {
            source,
            offset,
            prefix,
            file,
            marker,
            string_marker,
            source_map,
            classification: None,
            site_syntax: OnceLock::new(),
        };
        context.classification = context.classify_completion_context();
        Some(context)
    }

    fn string_marker(marker: &SyntaxToken) -> Option<SyntaxToken> {
        ast::String::cast(marker.clone())?
            .syntax()
            .text()
            .to_string()
            .contains(Self::MARKER)
            .then(|| marker.clone())
    }

    /// Returns the raw identifier prefix that should be replaced by a completion item.
    pub(super) fn prefix(&self) -> CompletionPrefix<'source> {
        self.prefix
    }

    /// Returns the identifier-like word inside a string, optionally retaining Cargo/ABI hyphens.
    ///
    /// Ordinary Rust prefix scanning intentionally stops at `-`. String resolvers need a wider
    /// spelling vocabulary, but that rule must not leak into normal identifiers such as `left-x`.
    pub(super) fn string_word_prefix(
        &self,
        allows_hyphen: bool,
    ) -> Option<CompletionPrefix<'source>> {
        self.string_word_prefix_impl(allows_hyphen)
    }

    /// Returns the original request-source range between the surrounding string quotes.
    pub(super) fn string_content_span(&self) -> Option<Span> {
        self.string_content_span_impl()
    }

    /// Returns the source range of the expression immediately before this completion dot.
    pub(super) fn postfix_receiver_span(&self) -> Option<Span> {
        self.postfix_receiver_span_impl()
    }

    pub(super) fn source_text(&self, span: Span) -> Option<&'source str> {
        self.source
            .get(usize::try_from(span.text.start).ok()?..usize::try_from(span.text.end).ok()?)
    }

    /// Returns the exact editor buffer represented by original-source spans.
    pub(super) fn source(&self) -> &'source str {
        self.source
    }

    /// Returns the completion domain computed while the speculative tree was built.
    pub(super) fn completion_context(&self) -> Option<&SyntaxCompletionContext> {
        self.classification.as_ref()
    }

    /// Returns all syntax-to-semantic routing facts computed with the request's one tree walk.
    pub(super) fn site_syntax(&self) -> CompletionSiteSyntax {
        self.site_syntax
            .get_or_init(|| self.classify_site_syntax())
            .clone()
    }

    fn classify_site_syntax(&self) -> CompletionSiteSyntax {
        let context = self.completion_context();
        let empty_path = match context {
            Some(SyntaxCompletionContext::EmptyPath(context)) => Some(*context),
            Some(
                SyntaxCompletionContext::Type(_)
                | SyntaxCompletionContext::Pattern(_)
                | SyntaxCompletionContext::ItemList(_)
                | SyntaxCompletionContext::BodyMacro(_)
                | SyntaxCompletionContext::ModuleMacro(_)
                | SyntaxCompletionContext::ModuleDeclaration(_)
                | SyntaxCompletionContext::Statement
                | SyntaxCompletionContext::Expression
                | SyntaxCompletionContext::Specialized(_),
            )
            | None => None,
        };
        let standalone = match context {
            Some(SyntaxCompletionContext::ItemList(context))
                if context.kind() == ItemListCompletionKind::TraitImpl =>
            {
                self.trait_impl_completion_syntax()
                    .map(StandaloneCompletionSiteSyntax::TraitImpl)
            }
            Some(SyntaxCompletionContext::BodyMacro(context)) => {
                Some(StandaloneCompletionSiteSyntax::BodyMacro {
                    qualifier: context.qualifier().cloned(),
                })
            }
            Some(SyntaxCompletionContext::ModuleMacro(context)) => {
                Some(StandaloneCompletionSiteSyntax::ModuleMacro {
                    qualifier: context.qualifier().cloned(),
                })
            }
            Some(SyntaxCompletionContext::ModuleDeclaration(context)) => {
                Some(StandaloneCompletionSiteSyntax::ModuleDeclaration {
                    has_path_attribute: context.has_path_attribute(),
                })
            }
            Some(
                SyntaxCompletionContext::EmptyPath(_)
                | SyntaxCompletionContext::Type(_)
                | SyntaxCompletionContext::Pattern(_)
                | SyntaxCompletionContext::ItemList(_)
                | SyntaxCompletionContext::Statement
                | SyntaxCompletionContext::Expression
                | SyntaxCompletionContext::Specialized(_),
            )
            | None => None,
        };
        let prefix = self.prefix();
        CompletionSiteSyntax::new(
            self.inside_use_item(),
            self.after_dot(),
            self.after_colon_colon(),
            self.empty_qualified_path(),
            empty_path,
            self.empty_record_owner(),
            self.body_owner_start(),
            standalone,
            prefix.span(),
            prefix.text().to_string(),
        )
    }

    /// Normalizes speculative parser ancestry into one owned completion-domain context.
    fn classify_completion_context(&self) -> Option<SyntaxCompletionContext> {
        if let Some(context) = self.specialized_completion_context() {
            return Some(SyntaxCompletionContext::Specialized(context));
        }
        if !self.accepts_completion_site() {
            return None;
        }

        // These forms are recognizable only in request-local syntax. They must be selected before
        // the generic item-list fallback, which would otherwise return only declaration keywords.
        if let Some(context) = self.module_declaration_context() {
            return Some(SyntaxCompletionContext::ModuleDeclaration(context));
        }
        if let Some(context) = self.module_macro_context() {
            return Some(SyntaxCompletionContext::ModuleMacro(context));
        }
        if let Some(context) = self.body_macro_context() {
            return Some(SyntaxCompletionContext::BodyMacro(context));
        }
        if self
            .marker
            .parent()?
            .ancestors()
            .any(|node| ast::MacroCall::can_cast(node.kind()))
        {
            return None;
        }

        // A missing member or path segment can look like an empty expression to parser recovery.
        // Preserve the decisive punctuation so the semantic dot/path scanners keep ownership.
        if self.after_dot() || self.after_colon_colon() {
            return None;
        }
        if let Some(context) = self.empty_path_completion_context() {
            return Some(SyntaxCompletionContext::EmptyPath(context));
        }
        if self.inside_use_item() {
            return None;
        }

        let mut pattern = None;
        for node in self.marker.parent()?.ancestors() {
            if ast::Type::can_cast(node.kind()) {
                return Some(SyntaxCompletionContext::Type(
                    Self::type_completion_context(&node),
                ));
            }

            if ast::RecordPat::can_cast(node.kind()) {
                pattern = Some(PatternCompletionKind::RecordConstructor);
            } else if ast::TupleStructPat::can_cast(node.kind()) {
                pattern = Some(PatternCompletionKind::TupleConstructor);
            } else if ast::Pat::can_cast(node.kind()) && pattern.is_none() {
                pattern = Some(PatternCompletionKind::Name);
            }

            if ast::AssocItemList::can_cast(node.kind()) {
                let owner = node.parent()?;
                let kind = if ast::Trait::can_cast(owner.kind()) {
                    ItemListCompletionKind::Trait
                } else {
                    let impl_ = ast::Impl::cast(owner)?;
                    if impl_.trait_().is_some() {
                        ItemListCompletionKind::TraitImpl
                    } else {
                        ItemListCompletionKind::InherentImpl
                    }
                };
                return Some(self.item_list_context(kind));
            }
            if ast::ExternItemList::can_cast(node.kind()) {
                let is_unsafe = node
                    .parent()
                    .and_then(ast::ExternBlock::cast)
                    .and_then(|block| block.unsafe_token())
                    .is_some();
                return Some(
                    self.item_list_context(ItemListCompletionKind::ExternBlock { is_unsafe }),
                );
            }
            if ast::ItemList::can_cast(node.kind()) {
                return Some(self.item_list_context(ItemListCompletionKind::Module));
            }
            if ast::StmtList::can_cast(node.kind()) {
                if let Some(kind) = pattern {
                    return Some(SyntaxCompletionContext::Pattern(kind));
                }

                let context = if self.at_statement_boundary() {
                    SyntaxCompletionContext::Statement
                } else {
                    SyntaxCompletionContext::Expression
                };
                return Some(context);
            }
            if ast::SourceFile::can_cast(node.kind()) {
                if let Some(kind) = pattern {
                    return Some(SyntaxCompletionContext::Pattern(kind));
                }
                return Some(self.item_list_context(ItemListCompletionKind::SourceFile));
            }
        }

        pattern.map(SyntaxCompletionContext::Pattern)
    }

    /// Returns true when the marker follows a plain dot access like `self.$0`.
    pub(super) fn after_dot(&self) -> bool {
        self.previous_non_trivia_token()
            .is_some_and(|token| token.kind() == SyntaxKind::DOT)
    }

    /// Returns true when the marker follows a path qualifier like `crate::$0`.
    pub(super) fn after_colon_colon(&self) -> bool {
        self.previous_non_trivia_token()
            .is_some_and(|token| token.kind() == SyntaxKind::COLON2)
    }

    /// Returns true when the marker lives syntactically inside a `use` item.
    pub(super) fn inside_use_item(&self) -> bool {
        self.marker.parent().is_some_and(|parent| {
            parent
                .ancestors()
                .any(|node| ast::Use::can_cast(node.kind()))
        })
    }

    /// Returns the nearest meaningful token before the speculative marker.
    pub(super) fn previous_non_trivia_token(&self) -> Option<SyntaxToken> {
        let mut token = self.marker.prev_token();
        while let Some(previous) = token {
            if !previous.kind().is_trivia() {
                return Some(previous);
            }
            token = previous.prev_token();
        }
        None
    }

    fn accepts_completion_site(&self) -> bool {
        if self.marker.text() != Self::MARKER || !self.marker.kind().is_any_identifier() {
            return false;
        }

        if self
            .previous_non_trivia_token()
            .is_some_and(|token| token.kind() == SyntaxKind::LIFETIME_IDENT)
        {
            return false;
        }

        true
    }

    /// Recognizes completion domains whose syntax is narrower than ordinary Rust identifiers.
    ///
    /// This runs before generic marker validation because lifetimes and string contents tokenize
    /// as one lifetime/string token rather than a standalone speculative identifier.
    fn specialized_completion_context(&self) -> Option<SpecializedCompletionContext> {
        if let Some(context) = self.attribute_completion_context() {
            return Some(SpecializedCompletionContext::Attribute(context));
        }
        if let Some(context) = self.string_completion_context() {
            return Some(SpecializedCompletionContext::String(context));
        }
        if let Some(context) = self.apostrophe_completion_context() {
            return Some(context);
        }
        if let Some(context) = self.visibility_completion_context() {
            return Some(SpecializedCompletionContext::RestrictedVisibility(context));
        }
        if self.marker.parent()?.ancestors().any(|node| {
            ast::ExternCrate::cast(node).is_some_and(|item| item.crate_token().is_some())
        }) {
            return Some(SpecializedCompletionContext::ExternCrateName);
        }
        if let Some(context) = self.const_expression_completion_context() {
            return Some(SpecializedCompletionContext::ConstExpression(context));
        }
        if self.is_macro_fragment_site() {
            return Some(SpecializedCompletionContext::MacroFragment);
        }
        None
    }

    /// Builds the one speculative tree shared by classification and source-edit planning.
    fn speculative_file(
        source: &str,
        prefix_start: usize,
        cursor: usize,
    ) -> Option<(SourceFile, SyntaxToken, CompletionSourceMap)> {
        let mut speculative =
            String::with_capacity(source.len() - (cursor - prefix_start) + Self::MARKER.len());
        speculative.push_str(source.get(..prefix_start)?);
        speculative.push_str(Self::MARKER);
        speculative.push_str(source.get(cursor..)?);

        // TODO: Thread the real package edition through completion syntax context.
        let file = SourceFile::parse(&speculative, Edition::CURRENT).tree();
        let marker_offset = TextSize::from(u32::try_from(prefix_start).ok()?);
        let marker = file
            .syntax()
            .token_at_offset(marker_offset)
            .right_biased()?;
        let marker_end = prefix_start.checked_add(Self::MARKER.len())?;
        let source_map = CompletionSourceMap {
            original_prefix: TextSpan {
                start: u32::try_from(prefix_start).ok()?,
                end: u32::try_from(cursor).ok()?,
            },
            speculative_marker: TextSpan {
                start: u32::try_from(prefix_start).ok()?,
                end: u32::try_from(marker_end).ok()?,
            },
        };
        Some((file, marker, source_map))
    }

    /// Returns the speculative file retained for source-aware completion edits.
    pub(super) fn speculative_file_tree(&self) -> &SourceFile {
        &self.file
    }

    /// Returns a cursor inside the speculative marker for syntax containment queries.
    pub(super) fn speculative_offset(&self) -> u32 {
        self.source_map.speculative_marker.end
    }

    /// Maps a syntax-tree range back into the exact request buffer.
    pub(super) fn original_span(&self, range: TextRange) -> Option<Span> {
        self.source_map.original_span(range)
    }

    fn prefix_start(source: &str, cursor: usize) -> usize {
        source[..cursor]
            .char_indices()
            .rev()
            .find(|(_, ch)| !Self::is_identifier_continue(*ch))
            .map(|(idx, ch)| idx + ch.len_utf8())
            .unwrap_or(0)
    }

    fn is_identifier_continue(ch: char) -> bool {
        ch == '_' || ch.is_ascii_alphanumeric()
    }
}

/// Offset translation for replacing the typed prefix with the fixed completion marker.
#[derive(Debug, Clone, Copy)]
struct CompletionSourceMap {
    original_prefix: TextSpan,
    speculative_marker: TextSpan,
}

impl CompletionSourceMap {
    fn original_span(self, range: TextRange) -> Option<Span> {
        let start = u32::from(range.start());
        let end = u32::from(range.end());
        let marker = self.speculative_marker;

        let mapped = if end <= marker.start {
            TextSpan { start, end }
        } else if marker.end <= start {
            TextSpan {
                start: self.after_marker_offset(start)?,
                end: self.after_marker_offset(end)?,
            }
        } else if start <= marker.start && marker.end <= end {
            TextSpan {
                start,
                end: self.after_marker_offset(end)?,
            }
        } else {
            // A range cutting through only part of the synthetic token has no faithful source
            // counterpart. Consumers should decline the edit rather than guess.
            return None;
        };
        Some(Span { text: mapped })
    }

    fn after_marker_offset(self, offset: u32) -> Option<u32> {
        let speculative_len =
            i64::from(self.speculative_marker.end - self.speculative_marker.start);
        let original_len = i64::from(self.original_prefix.end - self.original_prefix.start);
        u32::try_from(i64::from(offset) - speculative_len + original_len).ok()
    }
}

/// Identifier text and edit span already typed at the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompletionPrefix<'source> {
    text: &'source str,
    span: Span,
}

impl<'source> CompletionPrefix<'source> {
    pub(super) fn text(self) -> &'source str {
        self.text
    }

    pub(super) fn span(self) -> Span {
        self.span
    }

    pub(super) fn is_empty(self) -> bool {
        self.text.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use rg_ir_model::Path;

    use crate::query::completion::site::{
        BodyMacroCompletionContext, EmptyPathCompletionContext, ItemListCompletionContext,
        ItemListCompletionKind, ItemQualifierContext, ModuleDeclarationCompletionContext,
        ModuleMacroCompletionContext, RestrictedVisibilityCompletionContext,
        SpecializedCompletionContext, SpecializedStringCompletionContext, SyntaxCompletionContext,
        TraitImplMemberKind, TypeCompletionContext,
    };

    use super::CompletionSyntaxContext;

    #[test]
    fn computes_prefix_and_replacement_span() {
        let (source, offset) = source_with_cursor("fn main() {\n    let value = ma$0;\n}");
        let syntax = CompletionSyntaxContext::from_source(&source, offset)
            .expect("keyword syntax context should be created");
        let prefix = syntax.prefix();

        assert_eq!(prefix.text(), "ma");
        assert_eq!(prefix.span().text.start, 28);
        assert_eq!(prefix.span().text.end, 30);
    }

    #[test]
    fn retains_trait_impl_member_introducers_in_the_replacement_span() {
        let cases = [
            (
                "bare member",
                "trait Service {} struct Worker; impl Service for Worker { re$0 }",
                None,
                "re",
                None,
            ),
            (
                "function member",
                "trait Service {} struct Worker; impl Service for Worker { fn re$0 }",
                Some(TraitImplMemberKind::Function),
                "fn re",
                Some("fn "),
            ),
            (
                "qualified function member",
                "trait Service {} struct Worker; impl Service for Worker { async unsafe fn re$0 }",
                Some(TraitImplMemberKind::Function),
                "async unsafe fn re",
                Some("async unsafe fn "),
            ),
            (
                "type member",
                "trait Service {} struct Worker; impl Service for Worker { type Ou$0 }",
                Some(TraitImplMemberKind::TypeAlias),
                "type Ou",
                Some("type "),
            ),
            (
                "const member",
                "trait Service {} struct Worker; impl Service for Worker { const LI$0 }",
                Some(TraitImplMemberKind::Const),
                "const LI",
                Some("const "),
            ),
        ];

        for (label, source, expected_kind, expected_replacement, expected_lookup_prefix) in cases {
            let (source, offset) = source_with_cursor(source);
            let syntax = CompletionSyntaxContext::from_source(&source, offset)
                .expect("trait impl syntax context should be created");
            let trait_impl = syntax
                .trait_impl_completion_syntax()
                .expect("trait impl completion syntax should be recognized");

            assert_eq!(trait_impl.member_kind(), expected_kind, "{label}");
            assert_eq!(
                syntax.source_text(trait_impl.replace_span()),
                Some(expected_replacement),
                "{label}"
            );
            assert_eq!(
                trait_impl.lookup_prefix(),
                expected_lookup_prefix,
                "{label}"
            );
        }
    }

    #[test]
    fn classifies_completion_domains_from_speculative_syntax() {
        let cases = [
            (
                "source-file item",
                "f$0",
                Some((
                    SyntaxCompletionContext::ModuleMacro(ModuleMacroCompletionContext::incomplete(
                        None,
                        ItemListCompletionContext::new(
                            ItemListCompletionKind::SourceFile,
                            ItemQualifierContext::default(),
                        ),
                    )),
                    "f",
                )),
            ),
            (
                "module item",
                "mod inner { f$0 }",
                Some((
                    SyntaxCompletionContext::ModuleMacro(ModuleMacroCompletionContext::incomplete(
                        None,
                        ItemListCompletionContext::new(
                            ItemListCompletionKind::Module,
                            ItemQualifierContext::default(),
                        ),
                    )),
                    "f",
                )),
            ),
            (
                "inherent impl item",
                "struct S; impl S { f$0 }",
                Some((
                    SyntaxCompletionContext::ModuleMacro(ModuleMacroCompletionContext::incomplete(
                        None,
                        ItemListCompletionContext::new(
                            ItemListCompletionKind::InherentImpl,
                            ItemQualifierContext::default(),
                        ),
                    )),
                    "f",
                )),
            ),
            (
                "trait item",
                "trait Service { f$0 }",
                Some((
                    SyntaxCompletionContext::ModuleMacro(ModuleMacroCompletionContext::incomplete(
                        None,
                        ItemListCompletionContext::new(
                            ItemListCompletionKind::Trait,
                            ItemQualifierContext::default(),
                        ),
                    )),
                    "f",
                )),
            ),
            (
                "trait impl item",
                "trait Service {} struct S; impl Service for S { f$0 }",
                Some((
                    SyntaxCompletionContext::ItemList(ItemListCompletionContext::new(
                        ItemListCompletionKind::TraitImpl,
                        ItemQualifierContext::default(),
                    )),
                    "f",
                )),
            ),
            (
                "extern block item",
                "extern \"C\" { f$0 }",
                Some((
                    SyntaxCompletionContext::ItemList(ItemListCompletionContext::new(
                        ItemListCompletionKind::ExternBlock { is_unsafe: false },
                        ItemQualifierContext::default(),
                    )),
                    "f",
                )),
            ),
            (
                "unqualified module macro invocation",
                "build_ma$0!();",
                Some((
                    SyntaxCompletionContext::ModuleMacro(ModuleMacroCompletionContext::new(None)),
                    "build_ma",
                )),
            ),
            (
                "qualified module macro invocation",
                "tools::build_ma$0!();",
                Some((
                    SyntaxCompletionContext::ModuleMacro(ModuleMacroCompletionContext::new(Some(
                        Path::unqualified_name("tools"),
                    ))),
                    "build_ma",
                )),
            ),
            (
                "out-of-line module declaration",
                "mod pars$0;",
                Some((
                    SyntaxCompletionContext::ModuleDeclaration(
                        ModuleDeclarationCompletionContext::new(false),
                    ),
                    "pars",
                )),
            ),
            (
                "path-attributed module declaration",
                "#[path = \"custom.rs\"] mod pars$0;",
                Some((
                    SyntaxCompletionContext::ModuleDeclaration(
                        ModuleDeclarationCompletionContext::new(true),
                    ),
                    "pars",
                )),
            ),
            (
                "statement position",
                "fn main() {\n    le$0\n}",
                Some((SyntaxCompletionContext::Statement, "le")),
            ),
            (
                "expression position",
                "fn main() {\n    let _ = ma$0;\n}",
                Some((SyntaxCompletionContext::Expression, "ma")),
            ),
            (
                "bare expression position",
                "fn main() {\n    let _ = $0;\n}",
                Some((
                    SyntaxCompletionContext::EmptyPath(EmptyPathCompletionContext::Expression),
                    "",
                )),
            ),
            (
                "empty import path",
                "use $0;",
                Some((
                    SyntaxCompletionContext::EmptyPath(EmptyPathCompletionContext::Import),
                    "",
                )),
            ),
            (
                "empty type path",
                "fn inspect(_: $0) {}",
                Some((
                    SyntaxCompletionContext::EmptyPath(EmptyPathCompletionContext::Type),
                    "",
                )),
            ),
            (
                "empty call argument",
                "fn inspect() { consume($0); }",
                Some((
                    SyntaxCompletionContext::EmptyPath(EmptyPathCompletionContext::Argument),
                    "",
                )),
            ),
            (
                "empty generic argument",
                "fn inspect(_: Wrapper<$0>) {}",
                Some((
                    SyntaxCompletionContext::EmptyPath(EmptyPathCompletionContext::GenericArgument),
                    "",
                )),
            ),
            (
                "general type position",
                "struct S { field: d$0 }",
                Some((
                    SyntaxCompletionContext::Type(TypeCompletionContext::General),
                    "d",
                )),
            ),
            (
                "impl Trait function parameter",
                "fn inspect(value: i$0) {}",
                Some((
                    SyntaxCompletionContext::Type(TypeCompletionContext::ImplTraitAllowed),
                    "i",
                )),
            ),
        ];

        for (label, fixture, expected) in cases {
            let (source, offset) = source_with_cursor(fixture);
            let actual = CompletionSyntaxContext::from_source(&source, offset).and_then(|syntax| {
                Some((syntax.completion_context()?.clone(), syntax.prefix().text()))
            });

            assert_eq!(actual, expected, "{label}");
        }
    }

    #[test]
    fn preserves_item_qualifiers_and_specialized_positions() {
        let cases = [
            (
                "qualified module item",
                "pub(crate) async f$0",
                Some(SyntaxCompletionContext::ModuleMacro(
                    ModuleMacroCompletionContext::incomplete(
                        None,
                        ItemListCompletionContext::new(
                            ItemListCompletionKind::SourceFile,
                            ItemQualifierContext {
                                has_visibility: true,
                                has_async: true,
                                ..ItemQualifierContext::default()
                            },
                        ),
                    ),
                )),
            ),
            (
                "extern crate name",
                "extern crate dep$0;",
                Some(SyntaxCompletionContext::Specialized(
                    SpecializedCompletionContext::ExternCrateName,
                )),
            ),
            (
                "incomplete extern qualifier",
                "extern c$0",
                Some(SyntaxCompletionContext::ModuleMacro(
                    ModuleMacroCompletionContext::incomplete(
                        None,
                        ItemListCompletionContext::new(
                            ItemListCompletionKind::SourceFile,
                            ItemQualifierContext {
                                has_extern: true,
                                ..ItemQualifierContext::default()
                            },
                        ),
                    ),
                )),
            ),
            (
                "restricted visibility path",
                "pub(in crate::scope$0) struct S;",
                Some(SyntaxCompletionContext::Specialized(
                    SpecializedCompletionContext::RestrictedVisibility(
                        RestrictedVisibilityCompletionContext::new(
                            Path::from_macro_path_text("crate", None),
                            false,
                        ),
                    ),
                )),
            ),
            (
                "format capture string",
                "fn f() { let _ = format!(\"{na$0}\"); }",
                Some(SyntaxCompletionContext::Specialized(
                    SpecializedCompletionContext::String(
                        SpecializedStringCompletionContext::Format {
                            named_arguments: Vec::new(),
                        },
                    ),
                )),
            ),
            (
                "write destination string",
                "fn f() { let _ = write!(\"dest$0\", \"{}\"); }",
                None,
            ),
            (
                "write format string",
                "fn f() { let _ = write!(out, \"{na$0}\"); }",
                Some(SyntaxCompletionContext::Specialized(
                    SpecializedCompletionContext::String(
                        SpecializedStringCompletionContext::Format {
                            named_arguments: Vec::new(),
                        },
                    ),
                )),
            ),
            (
                "Cargo environment string",
                "fn f() { let _ = env!(\"CARGO_MAN$0\"); }",
                Some(SyntaxCompletionContext::Specialized(
                    SpecializedCompletionContext::String(
                        SpecializedStringCompletionContext::Environment,
                    ),
                )),
            ),
            (
                "extern ABI string",
                "extern \"C-un$0\" fn f();",
                Some(SyntaxCompletionContext::Specialized(
                    SpecializedCompletionContext::String(SpecializedStringCompletionContext::Abi),
                )),
            ),
            (
                "macro fragment",
                "macro_rules! m { ($value: ex$0) => {}; }",
                Some(SyntaxCompletionContext::Specialized(
                    SpecializedCompletionContext::MacroFragment,
                )),
            ),
        ];

        for (label, fixture, expected) in cases {
            let (source, offset) = source_with_cursor(fixture);
            let actual = CompletionSyntaxContext::from_source(&source, offset)
                .and_then(|syntax| syntax.completion_context().cloned());

            assert_eq!(actual, expected, "{label}");
        }
    }

    #[test]
    fn rejects_keyword_sites_inside_non_code_syntax() {
        let cases = [
            ("line comment", "fn main() {\n    // ma$0\n}"),
            ("block comment", "fn main() {\n    /* ma$0 */\n}"),
            ("string literal", r#"fn main() { let _ = "ma$0"; }"#),
            (
                "raw string literal",
                r##"fn main() { let _ = r#"ma$0"#; }"##,
            ),
            ("use item", "use ma$0;"),
            ("field access", "fn main() { value.ma$0 }"),
            ("empty field access", "fn main() { value.$0 }"),
            ("path qualifier", "fn main() { crate::ma$0 }"),
            ("empty path segment", "fn main() { crate::$0 }"),
        ];

        for (label, fixture) in cases {
            let (source, offset) = source_with_cursor(fixture);
            let actual = CompletionSyntaxContext::from_source(&source, offset)
                .and_then(|syntax| syntax.completion_context().cloned());

            assert_eq!(actual, None, "{label}");
        }

        let (source, offset) = source_with_cursor("fn main() { crate::macros::item_ma$0!(); }");
        let actual = CompletionSyntaxContext::from_source(&source, offset)
            .and_then(|syntax| syntax.completion_context().cloned());
        assert_eq!(
            actual,
            Some(SyntaxCompletionContext::BodyMacro(
                BodyMacroCompletionContext::new(Some(
                    Path::from_macro_path_text("crate::macros", None)
                        .expect("qualified macro path should parse"),
                )),
            )),
            "body macro callee"
        );
    }

    #[test]
    fn exposes_common_token_neighborhood_predicates() {
        let cases = [
            ("dot access", "fn main() { self.$0 }", true, false, false),
            (
                "path qualifier",
                "fn main() { crate::$0 }",
                false,
                true,
                false,
            ),
            ("use path", "use std::collections::$0;", false, true, true),
        ];

        for (label, fixture, after_dot, after_colon_colon, inside_use) in cases {
            let (source, offset) = source_with_cursor(fixture);
            let syntax = CompletionSyntaxContext::from_source(&source, offset)
                .expect("syntax context should be created");

            assert_eq!(syntax.after_dot(), after_dot, "{label}: after_dot");
            assert_eq!(
                syntax.after_colon_colon(),
                after_colon_colon,
                "{label}: after_colon_colon"
            );
            assert_eq!(syntax.inside_use_item(), inside_use, "{label}: use item");
        }
    }

    #[test]
    fn recovers_empty_record_and_path_triggers_at_file_end() {
        let (source, offset) = source_with_cursor("fn main() { let _ = crate::Record { $0");
        let syntax = CompletionSyntaxContext::from_source(&source, offset)
            .expect("record syntax context should be created");
        assert_eq!(
            syntax.empty_record_owner().map(|path| path.to_string()),
            Some("crate::Record".to_string()),
        );

        let (source, offset) = source_with_cursor("fn main() { let _: crate::api::$0");
        let syntax = CompletionSyntaxContext::from_source(&source, offset)
            .expect("path syntax context should be created");
        assert!(syntax.after_colon_colon());
        assert_eq!(
            syntax
                .empty_qualified_path()
                .map(|path| path.qualifier().to_string()),
            Some("crate::api".to_string())
        );
    }

    #[test]
    fn preserves_specialized_content_and_receiver_ranges() {
        let (source, offset) =
            source_with_cursor("fn f() { let _ = env!(\"CARGO-MANIFEST-DIRECTORY$0\"); }");
        let syntax = CompletionSyntaxContext::from_source(&source, offset)
            .expect("environment string syntax should be created");
        let string_prefix = syntax
            .string_word_prefix(true)
            .expect("hyphenated string prefix should be available");
        assert_eq!(string_prefix.text(), "CARGO-MANIFEST-DIRECTORY");
        let content = syntax
            .string_content_span()
            .expect("string content range should be available");
        assert_eq!(
            syntax.source_text(content),
            Some("CARGO-MANIFEST-DIRECTORY")
        );

        let (source, offset) =
            source_with_cursor("fn f(left: i32, right: i32) { (left + right).ma$0; }");
        let syntax = CompletionSyntaxContext::from_source(&source, offset)
            .expect("postfix syntax should be created");
        let receiver = syntax
            .postfix_receiver_span()
            .expect("postfix receiver should be available");
        assert_eq!(syntax.source_text(receiver), Some("(left + right)"));
    }

    fn source_with_cursor(fixture: &str) -> (String, u32) {
        let offset = fixture
            .find("$0")
            .expect("syntax fixture should include a cursor marker");
        let mut source = fixture.to_string();
        source.replace_range(offset..offset + "$0".len(), "");
        (
            source,
            u32::try_from(offset).expect("syntax fixture offset should fit into u32"),
        )
    }
}
