//! Parse fenced examples as Rust and translate their token ranges back into Markdown.

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
use rg_parse::{TextRangeMap, TextRangeMapping};
use rg_syntax::{AstNode as _, Edition, SourceFile, SyntaxKind, SyntaxToken, ast};

use crate::{Analysis, HighlightKind, SymbolKind};

/// Give one Rust example enough syntax context to color statements as well as declarations.
///
/// An example such as `let profile = load();` is a statement, so `source` wraps it in a
/// synthetic function body. Only bytes copied from the Markdown get mappings. The function
/// wrapper can help parsing without producing highlights in the user's documentation.
pub(super) struct CodeBlock {
    source: String,
    edition: Edition,
    mappings: TextRangeMap,
}

impl CodeBlock {
    /// Markdown owns fence boundaries and indentation. Each example gets its own Rust context.
    /// Extract recognized Rust fences from the full document, retaining where their contents
    /// appeared before Markdown removed indentation or block-quote prefixes.
    pub(super) fn extract(markdown: &str, edition: Edition) -> impl Iterator<Item = Self> + '_ {
        let mut current: Option<Self> = None;
        Parser::new(markdown)
            .into_offset_iter()
            .filter_map(move |(event, range)| {
                match event {
                    Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) => {
                        current = Self::rust_edition(&info, edition).map(|edition| Self {
                            source: "fn __doc_example() {\n".into(),
                            edition,
                            mappings: TextRangeMap::default(),
                        });
                    }
                    Event::Text(text) => {
                        if let Some(block) = &mut current {
                            // A code-text event may omit a block quote or indentation from each line.
                            // Match within this event's original range, advancing monotonically.
                            let mut cursor = range.start;
                            for line in text.split_inclusive('\n') {
                                if let Some(relative) = markdown[cursor..range.end].find(line) {
                                    let start = cursor + relative;
                                    block.push_line(line, start);
                                    cursor = start + line.len();
                                } else {
                                    // Parser-generated whitespace has no exact authored token range.
                                    block.source.push_str(line);
                                }
                            }
                        }
                    }
                    Event::End(TagEnd::CodeBlock) => {
                        if let Some(mut block) = current.take() {
                            block.source.push_str("\n}");
                            return Some(block);
                        }
                    }
                    _ => {}
                }
                None
            })
    }

    /// Rustdoc uses a bare fence for Rust and accepts test flags in place of a language name.
    fn rust_edition(info: &str, mut edition: Edition) -> Option<Edition> {
        let mut explicit_rust = false;
        let mut other = false;
        for tag in info.split([',', ' ', '\t']).filter(|tag| !tag.is_empty()) {
            match tag {
                "rust" => explicit_rust = true,
                "edition2015" => edition = Edition::Edition2015,
                "edition2018" => edition = Edition::Edition2018,
                "edition2021" => edition = Edition::Edition2021,
                "edition2024" => edition = Edition::Edition2024,
                "no_run" | "ignore" | "should_panic" | "compile_fail" | "test_harness"
                | "allow_fail" | "standalone_crate" => {}
                tag if tag.starts_with("ignore-") => {}
                tag if tag.len() == 5
                    && tag.starts_with('E')
                    && tag[1..].bytes().all(|byte| byte.is_ascii_digit()) => {}
                _ => other = true,
            }
        }
        (explicit_rust || !other).then_some(edition)
    }

    /// Append an example line that starts at byte `offset` in the complete Markdown document.
    ///
    /// Hidden lines still contain Rust in source. Remove only their marker from the parser input;
    /// `##` escapes a literal hash, so it also loses exactly one hash.
    fn push_line(&mut self, line: &str, offset: usize) {
        let trimmed = line.trim_start();
        let hash = if trimmed.trim_end() == "#"
            || trimmed.starts_with("# ")
            || trimmed.starts_with("#\t")
            || trimmed.starts_with("##")
        {
            Some(line.len() - trimmed.len())
        } else {
            None
        };
        if let Some(hash) = hash {
            self.push(&line[..hash], offset);
            self.push(&line[hash + 1..], offset + hash + 1);
        } else {
            self.push(line, offset);
        }
    }

    /// Record text copied verbatim from Markdown. Wrapper text and parser-generated padding
    /// are appended without this mapping because they have no authored bytes to highlight.
    fn push(&mut self, text: &str, offset: usize) {
        let start = self.source.len();
        self.source.push_str(text);
        self.mappings.push(TextRangeMapping::copied(
            start..self.source.len(),
            offset..offset + text.len(),
        ));
    }

    /// Color the example's syntax and return byte ranges in the complete Markdown document.
    /// A token can cross several copied pieces, so intersect it with each mapping. This also
    /// drops tokens belonging only to the wrapper; Rust file positions are recovered separately
    /// from the documentation's mappings.
    pub(super) fn highlights(
        &self,
        analysis: &Analysis<'_>,
    ) -> anyhow::Result<Vec<(Range<usize>, HighlightKind)>> {
        let syntax = SourceFile::parse(&self.source, self.edition).tree();
        let mut highlights = Vec::new();
        for token in syntax
            .syntax()
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
        {
            rg_std::check_cancel!(analysis, "example syntax token");
            let Some(kind) = Self::token_kind(&token) else {
                continue;
            };
            let range =
                usize::from(token.text_range().start())..usize::from(token.text_range().end());
            for mapping in self.mappings.project(range) {
                highlights.push((mapping.original, kind));
            }
        }
        Ok(highlights)
    }

    /// Choose colors from syntax roles without resolving names or inferring example types.
    /// For example, `load` in `profile.load()` gets a method color without knowing the type of
    /// `profile`. Examples can therefore be colored even when their names have no indexed owners.
    fn token_kind(token: &SyntaxToken) -> Option<HighlightKind> {
        use SyntaxKind::{
            BYTE, BYTE_STRING, C_STRING, CHAR, COMMENT, FLOAT_NUMBER, IDENT, INT_NUMBER,
            LIFETIME_IDENT, STRING,
        };
        let kind = token.kind();
        if kind.is_keyword(Edition::CURRENT) {
            return Some(HighlightKind::Keyword);
        }
        Some(match kind {
            COMMENT => HighlightKind::Comment,
            STRING | BYTE_STRING | C_STRING | CHAR | BYTE => HighlightKind::String,
            INT_NUMBER | FLOAT_NUMBER => HighlightKind::Number,
            LIFETIME_IDENT => HighlightKind::TypeParameter,
            IDENT => {
                let parent = token.parent()?;
                let owner = parent.parent();
                if ast::Name::can_cast(parent.kind()) {
                    match owner?.kind() {
                        SyntaxKind::FN => HighlightKind::Symbol(SymbolKind::Function),
                        SyntaxKind::STRUCT => HighlightKind::Symbol(SymbolKind::Struct),
                        SyntaxKind::ENUM => HighlightKind::Symbol(SymbolKind::Enum),
                        SyntaxKind::TRAIT => HighlightKind::Symbol(SymbolKind::Trait),
                        SyntaxKind::TYPE_ALIAS => HighlightKind::Symbol(SymbolKind::TypeAlias),
                        SyntaxKind::MODULE => HighlightKind::Symbol(SymbolKind::Module),
                        SyntaxKind::CONST => HighlightKind::Symbol(SymbolKind::Const),
                        SyntaxKind::STATIC => HighlightKind::Symbol(SymbolKind::Static),
                        SyntaxKind::RECORD_FIELD => HighlightKind::Symbol(SymbolKind::Field),
                        SyntaxKind::VARIANT => HighlightKind::Symbol(SymbolKind::EnumVariant),
                        SyntaxKind::TYPE_PARAM => HighlightKind::TypeParameter,
                        _ if parent
                            .ancestors()
                            .any(|node| ast::Param::can_cast(node.kind())) =>
                        {
                            HighlightKind::Parameter
                        }
                        _ => HighlightKind::Symbol(SymbolKind::Variable),
                    }
                } else if token
                    .parent_ancestors()
                    .any(|node| ast::PathType::can_cast(node.kind()))
                {
                    HighlightKind::Symbol(SymbolKind::TypeAlias)
                } else if owner
                    .as_ref()
                    .is_some_and(|node| ast::MethodCallExpr::can_cast(node.kind()))
                {
                    HighlightKind::Symbol(SymbolKind::Method)
                } else {
                    // Only the final path segment can be the called function or macro.
                    let path = token.parent_ancestors().find_map(ast::Path::cast);
                    let path_owner = path.and_then(|path| path.syntax().parent());
                    if path_owner
                        .as_ref()
                        .is_some_and(|node| ast::MacroCall::can_cast(node.kind()))
                    {
                        HighlightKind::Symbol(SymbolKind::Macro)
                    } else if path_owner
                        .and_then(|node| node.parent())
                        .is_some_and(|node| ast::CallExpr::can_cast(node.kind()))
                    {
                        HighlightKind::Symbol(SymbolKind::Function)
                    } else {
                        HighlightKind::Symbol(SymbolKind::Variable)
                    }
                }
            }
            _ if kind.is_punct() => HighlightKind::Operator,
            _ => return None,
        })
    }
}
