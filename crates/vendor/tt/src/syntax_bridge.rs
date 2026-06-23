//! Conversion between rust-glancer syntax trees and macro token trees.
//!
//! Macro expansion should not be lowered through source text. The bridge below feeds token trees
//! directly into the parser and then builds a frozen syntax tree from parser events and known token
//! text. This preserves token jointness and allows special tokens such as `$crate` to remain
//! syntax-level facts instead of string rendering conventions.

use std::collections::HashMap;

use parser::{Edition, SyntaxKind, T};
use rg_syntax::{
    AstNode as _, GeneratedSyntaxBuilder, NodeOrToken, Parse, SyntaxNode, SyntaxToken, TextRange,
    TextSize, ast,
};
use smol_str::SmolStr;

use crate::{
    span::{EditionedFileId, ROOT_ERASED_FILE_AST_ID, Span, SpanAnchor, SyntaxContext},
    tt::{
        self, DelimiterKind, Leaf, Spacing, TokenTree, TokenTreesView, buffer::Cursor,
        symbol::Symbol,
    },
};

/// Mapping from offsets in generated syntax back to token-tree spans.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExpansionSpanMap {
    spans: Vec<(TextSize, Span)>,
}

impl ExpansionSpanMap {
    pub fn span_at(&self, offset: TextSize) -> Option<Span> {
        let index = self.spans.partition_point(|(end, _)| *end <= offset);
        self.spans
            .get(index)
            .or_else(|| self.spans.last())
            .map(|(_, span)| *span)
    }

    pub fn span_for_range(&self, range: TextRange) -> Option<Span> {
        self.span_at(range.start())
    }

    /// Returns the mapped span only when it points into the requested source file.
    pub fn span_for_range_in_file(&self, range: TextRange, origin_file_id: usize) -> Option<Span> {
        let span = self.span_for_range(range)?;
        (span.anchor.file_id.raw_file_id() as usize == origin_file_id).then_some(span)
    }

    fn push(&mut self, end: TextSize, span: Span) {
        self.spans.push((end, span));
    }
}

/// Produces coarse spans for syntax converted into token trees.
#[derive(Clone, Copy)]
pub struct SpanFactory {
    anchor: SpanAnchor,
    ctx: SyntaxContext,
}

impl SpanFactory {
    pub fn new(file_id: u32, edition: Edition) -> Self {
        Self {
            anchor: SpanAnchor {
                file_id: EditionedFileId::new(file_id, edition),
                ast_id: ROOT_ERASED_FILE_AST_ID,
            },
            ctx: SyntaxContext::root(edition),
        }
    }

    pub fn span_for(self, range: TextRange) -> Span {
        Span {
            range,
            anchor: self.anchor,
            ctx: self.ctx,
        }
    }
}

/// Converts an `rg_syntax` token-tree AST node into the compact token-tree format.
pub fn syntax_node_to_token_tree(
    tree: &ast::TokenTree,
    span_factory: SpanFactory,
) -> tt::TopSubtree {
    let mut span_for_range = |range| span_factory.span_for(range);
    syntax_node_to_token_tree_with_span(tree, &mut span_for_range)
}

/// Converts syntax into token trees with caller-provided source mapping.
///
/// Source files use direct file ranges, while generated syntax maps through the expansion span map.
/// Keeping the mapper at this boundary lets item-tree store token trees without reconstructing
/// temporary source snippets later.
pub fn syntax_node_to_token_tree_with_span(
    tree: &ast::TokenTree,
    span_for_range: &mut dyn FnMut(TextRange) -> Span,
) -> tt::TopSubtree {
    let delimiter = delimiter_for(tree, span_for_range);
    let mut builder = tt::TopSubtreeBuilder::new(delimiter);
    push_token_tree_children(&mut builder, tree, span_for_range);
    builder.build()
}

/// Parses an expanded token tree without first rendering it to Rust source text.
pub fn token_tree_to_syntax_node(
    tt: &tt::TopSubtree,
    entry_point: parser::TopEntryPoint,
    span_to_edition: &mut dyn FnMut(SyntaxContext) -> Edition,
) -> (Parse<SyntaxNode>, ExpansionSpanMap) {
    let buffer = tt.view().strip_invisible();
    let parser_input = to_parser_input(buffer, span_to_edition);
    let parser_output = entry_point.parse(&parser_input);
    let mut tree_sink = TtTreeSink::new(buffer.cursor());

    for event in parser_output.iter() {
        match event {
            parser::Step::Token {
                kind,
                n_input_tokens,
            } => tree_sink.token(kind, n_input_tokens),
            parser::Step::FloatSplit { ends_in_dot } => tree_sink.float_split(ends_in_dot),
            parser::Step::Enter { kind } => tree_sink.start_node(kind),
            parser::Step::Exit => tree_sink.finish_node(),
            parser::Step::Error { msg } => tree_sink.error(msg.to_owned()),
        }
    }

    tree_sink.finish()
}

/// Builds parser input from token trees while preserving punctuation jointness.
pub fn to_parser_input(
    buffer: TokenTreesView<'_>,
    ctx_edition: &mut dyn FnMut(SyntaxContext) -> Edition,
) -> parser::Input {
    let mut input = parser::Input::with_capacity(buffer.len());
    let mut current = buffer.cursor();
    let mut context_cache = HashMap::new();

    while !current.eof() {
        let tt = current.token_tree();

        if let Some(TokenTree::Leaf(Leaf::Punct(punct))) = tt
            && punct.char == '\''
        {
            current.bump();
            match current.token_tree() {
                Some(TokenTree::Leaf(Leaf::Ident(ident))) => {
                    input.push(
                        SyntaxKind::LIFETIME_IDENT,
                        edition_for(ident.span.ctx, &mut context_cache, ctx_edition),
                    );
                    current.bump();
                    continue;
                }
                _ => panic!("next token after lifetime quote must be an ident"),
            }
        }

        match tt {
            Some(TokenTree::Leaf(leaf)) => {
                match leaf {
                    Leaf::Literal(lit) => {
                        let kind = match lit.kind {
                            tt::LitKind::Byte => SyntaxKind::BYTE,
                            tt::LitKind::Char => SyntaxKind::CHAR,
                            tt::LitKind::Integer => SyntaxKind::INT_NUMBER,
                            tt::LitKind::Float => SyntaxKind::FLOAT_NUMBER,
                            tt::LitKind::Str | tt::LitKind::StrRaw(_) => SyntaxKind::STRING,
                            tt::LitKind::ByteStr | tt::LitKind::ByteStrRaw(_) => {
                                SyntaxKind::BYTE_STRING
                            }
                            tt::LitKind::CStr | tt::LitKind::CStrRaw(_) => SyntaxKind::C_STRING,
                            tt::LitKind::Err(_) => SyntaxKind::ERROR,
                        };
                        input.push(
                            kind,
                            edition_for(lit.span.ctx, &mut context_cache, ctx_edition),
                        );

                        if kind == SyntaxKind::FLOAT_NUMBER && !lit.text().ends_with('.') {
                            input.was_joint();
                        }
                    }
                    Leaf::Ident(ident) => {
                        let edition = edition_for(ident.span.ctx, &mut context_cache, ctx_edition);
                        match ident.sym.as_str() {
                            "_" => input.push(T![_], edition),
                            text if text.starts_with('\'') => {
                                input.push(SyntaxKind::LIFETIME_IDENT, edition)
                            }
                            _ if ident.is_raw.yes() => input.push(SyntaxKind::IDENT, edition),
                            text => match SyntaxKind::from_keyword(text, edition) {
                                Some(kind) => input.push(kind, edition),
                                None => {
                                    let contextual_keyword =
                                        SyntaxKind::from_contextual_keyword(text, edition)
                                            .unwrap_or(SyntaxKind::IDENT);
                                    input.push_ident(contextual_keyword, edition);
                                }
                            },
                        }
                    }
                    Leaf::Punct(punct) => {
                        let kind = SyntaxKind::from_char(punct.char)
                            .unwrap_or_else(|| panic!("{punct:#?} is not a valid punct"));
                        input.push(
                            kind,
                            edition_for(punct.span.ctx, &mut context_cache, ctx_edition),
                        );
                        if punct.spacing == Spacing::Joint {
                            input.was_joint();
                        }
                    }
                }
                current.bump();
            }
            Some(TokenTree::Subtree(subtree)) => {
                if let Some(kind) = delimiter_token_kind(subtree.delimiter.kind, false) {
                    input.push(
                        kind,
                        edition_for(subtree.delimiter.open.ctx, &mut context_cache, ctx_edition),
                    );
                }
                current.bump();
            }
            None => {
                let subtree = current.end();
                if let Some(kind) = delimiter_token_kind(subtree.delimiter.kind, true) {
                    input.push(
                        kind,
                        edition_for(subtree.delimiter.close.ctx, &mut context_cache, ctx_edition),
                    );
                }
            }
        };
    }

    input
}

fn edition_for(
    ctx: SyntaxContext,
    context_cache: &mut HashMap<SyntaxContext, Edition>,
    ctx_edition: &mut dyn FnMut(SyntaxContext) -> Edition,
) -> Edition {
    *context_cache.entry(ctx).or_insert_with(|| ctx_edition(ctx))
}

fn delimiter_token_kind(kind: DelimiterKind, closing: bool) -> Option<SyntaxKind> {
    match (kind, closing) {
        (DelimiterKind::Parenthesis, false) => Some(T!['(']),
        (DelimiterKind::Parenthesis, true) => Some(T![')']),
        (DelimiterKind::Brace, false) => Some(T!['{']),
        (DelimiterKind::Brace, true) => Some(T!['}']),
        (DelimiterKind::Bracket, false) => Some(T!['[']),
        (DelimiterKind::Bracket, true) => Some(T![']']),
        (DelimiterKind::Invisible, _) => None,
    }
}

struct TtTreeSink<'a> {
    cursor: Cursor<'a>,
    inner: GeneratedSyntaxBuilder,
    token_map: ExpansionSpanMap,
}

impl<'a> TtTreeSink<'a> {
    fn new(cursor: Cursor<'a>) -> Self {
        Self {
            cursor,
            inner: GeneratedSyntaxBuilder::new(),
            token_map: ExpansionSpanMap::default(),
        }
    }

    fn finish(self) -> (Parse<SyntaxNode>, ExpansionSpanMap) {
        (self.inner.finish(), self.token_map)
    }

    fn float_split(&mut self, has_pseudo_dot: bool) {
        let (text, span) = match self.cursor.token_tree() {
            Some(TokenTree::Leaf(Leaf::Literal(lit))) if lit.kind == tt::LitKind::Float => {
                (lit.to_string(), lit.span)
            }
            tt => unreachable!("{tt:?}"),
        };

        let Some((left, right)) = text.split_once('.') else {
            unreachable!();
        };
        assert!(!left.is_empty());

        self.inner.start_node(SyntaxKind::NAME_REF);
        self.inner.token(SyntaxKind::INT_NUMBER, left);
        self.token_map.push(self.inner.current_offset(), span);
        self.inner.finish_node();

        // The parser split a single float token into field-access syntax. The original event stream
        // has already accounted for one synthetic node exit, so the sink mirrors that shape here.
        self.inner.finish_node();

        self.inner.token(SyntaxKind::DOT, ".");
        self.token_map.push(self.inner.current_offset(), span);

        if has_pseudo_dot {
            assert!(right.is_empty(), "{left}.{right}");
        } else {
            assert!(!right.is_empty(), "{left}.{right}");
            self.inner.start_node(SyntaxKind::NAME_REF);
            self.inner.token(SyntaxKind::INT_NUMBER, right);
            self.token_map.push(self.inner.current_offset(), span);
            self.inner.finish_node();
            self.inner.finish_node();
        }

        self.cursor.bump();
    }

    fn token(&mut self, kind: SyntaxKind, mut n_tokens: u8) {
        if kind == SyntaxKind::LIFETIME_IDENT {
            n_tokens = 2;
        }

        let mut text = String::new();
        let mut combined_span = None;
        let mut last_two = self.cursor.peek_two_leaves();

        for _ in 0..n_tokens {
            if self.cursor.eof() {
                break;
            }
            last_two = self.cursor.peek_two_leaves();
            let (token_text, span) = self.next_token_text();
            text.push_str(token_text.as_str());
            combined_span = Some(match combined_span {
                None => span,
                Some(prev_span) => Self::merge_spans(prev_span, span),
            });
        }

        let span = combined_span.expect("parser token should consume at least one token tree");
        self.inner.token(kind, text.as_str());
        self.token_map.push(self.inner.current_offset(), span);

        // The syntax tree stores token text in one contiguous string. Add a small amount of trivia
        // after non-joint tokens so materializing that text does not merge neighboring tokens into a
        // different token stream.
        if let Some([current, next]) = last_two
            && needs_space_after(&current, &next)
        {
            self.inner.token(SyntaxKind::WHITESPACE, " ");
            self.token_map
                .push(self.inner.current_offset(), *current.span());
        }
    }

    fn next_token_text(&mut self) -> (SmolStr, Span) {
        match self.cursor.token_tree() {
            Some(TokenTree::Leaf(Leaf::Ident(ident))) => {
                let text = if ident.is_raw.yes() {
                    SmolStr::new(format!("r#{}", ident.sym))
                } else {
                    SmolStr::new(ident.sym.as_str())
                };
                self.cursor.bump();
                (text, ident.span)
            }
            Some(TokenTree::Leaf(Leaf::Punct(punct))) => {
                let mut bytes = [0; 4];
                let text = SmolStr::new_inline(punct.char.encode_utf8(&mut bytes));
                self.cursor.bump();
                (text, punct.span)
            }
            Some(TokenTree::Leaf(Leaf::Literal(lit))) => {
                let text = SmolStr::new(lit.to_string().as_str());
                self.cursor.bump();
                (text, lit.span)
            }
            Some(TokenTree::Subtree(subtree)) => {
                self.cursor.bump();
                let Some(text) = delimiter_token_text(subtree.delimiter.kind, false) else {
                    return self.next_token_text();
                };
                (SmolStr::new_inline(text), subtree.delimiter.open)
            }
            None => {
                let subtree = self.cursor.end();
                let Some(text) = delimiter_token_text(subtree.delimiter.kind, true) else {
                    return self.next_token_text();
                };
                (SmolStr::new_inline(text), subtree.delimiter.close)
            }
        }
    }

    fn start_node(&mut self, kind: SyntaxKind) {
        self.inner.start_node(kind);
    }

    fn finish_node(&mut self) {
        self.inner.finish_node();
    }

    fn error(&mut self, error: String) {
        self.inner.error(error);
    }

    fn merge_spans(left: Span, right: Span) -> Span {
        Span {
            range: if left.ctx == right.ctx && left.anchor == right.anchor {
                TextRange::new(
                    left.range.start().min(right.range.start()),
                    left.range.end().max(right.range.end()),
                )
            } else {
                left.range
            },
            anchor: left.anchor,
            ctx: left.ctx,
        }
    }
}

fn needs_space_after(current: &Leaf, next: &Leaf) -> bool {
    match current {
        Leaf::Punct(punct) => {
            punct.spacing == Spacing::Alone
                && punct.char != ';'
                && !matches!(next, Leaf::Punct(next) if next.char == '\'')
        }
        Leaf::Ident(_) | Leaf::Literal(_) => true,
    }
}

fn delimiter_token_text(kind: DelimiterKind, closing: bool) -> Option<&'static str> {
    match (kind, closing) {
        (DelimiterKind::Parenthesis, false) => Some("("),
        (DelimiterKind::Parenthesis, true) => Some(")"),
        (DelimiterKind::Brace, false) => Some("{"),
        (DelimiterKind::Brace, true) => Some("}"),
        (DelimiterKind::Bracket, false) => Some("["),
        (DelimiterKind::Bracket, true) => Some("]"),
        (DelimiterKind::Invisible, _) => None,
    }
}

fn push_nested_token_tree(
    builder: &mut tt::TopSubtreeBuilder,
    tree: &ast::TokenTree,
    span_for_range: &mut dyn FnMut(TextRange) -> Span,
) {
    let delimiter = delimiter_for(tree, span_for_range);
    builder.open(delimiter.kind, delimiter.open);
    push_token_tree_children(builder, tree, span_for_range);
    builder.close(delimiter.close);
}

fn push_token_tree_children(
    builder: &mut tt::TopSubtreeBuilder,
    tree: &ast::TokenTree,
    span_for_range: &mut dyn FnMut(TextRange) -> Span,
) {
    let left = tree.left_delimiter_token().map(|token| token.text_range());
    let right = tree.right_delimiter_token().map(|token| token.text_range());
    let children = tree.token_trees_and_tokens().collect::<Vec<_>>();

    for (index, child) in children.iter().enumerate() {
        match child {
            NodeOrToken::Node(tree) => push_nested_token_tree(builder, tree, span_for_range),
            NodeOrToken::Token(token)
                if Some(token.text_range()) == left || Some(token.text_range()) == right => {}
            NodeOrToken::Token(token) => {
                let joint_with_next =
                    next_punctuation_is_adjacent(token, &children[index.saturating_add(1)..]);
                push_token(builder, token, span_for_range, joint_with_next);
            }
        }
    }
}

fn next_punctuation_is_adjacent(
    token: &SyntaxToken,
    following: &[NodeOrToken<ast::TokenTree, SyntaxToken>],
) -> bool {
    if !token.kind().is_punct() {
        return false;
    }

    for child in following {
        match child {
            NodeOrToken::Token(next_token) if next_token.kind().is_trivia() => continue,
            NodeOrToken::Token(next_token) => {
                return next_token.kind().is_punct()
                    && token.text_range().end() == next_token.text_range().start();
            }
            NodeOrToken::Node(_) => return false,
        }
    }

    false
}

fn delimiter_for(
    tree: &ast::TokenTree,
    span_for_range: &mut dyn FnMut(TextRange) -> Span,
) -> tt::Delimiter {
    let Some(left) = tree.left_delimiter_token() else {
        return tt::Delimiter::invisible_spanned(span_for_range(tree.syntax().text_range()));
    };
    let Some(right) = tree.right_delimiter_token() else {
        return tt::Delimiter::invisible_spanned(span_for_range(tree.syntax().text_range()));
    };

    let kind = match left.kind() {
        T!['('] => DelimiterKind::Parenthesis,
        T!['{'] => DelimiterKind::Brace,
        T!['['] => DelimiterKind::Bracket,
        _ => DelimiterKind::Invisible,
    };

    tt::Delimiter {
        open: span_for_range(left.text_range()),
        close: span_for_range(right.text_range()),
        kind,
    }
}

fn push_token(
    builder: &mut tt::TopSubtreeBuilder,
    token: &SyntaxToken,
    span_for_range: &mut dyn FnMut(TextRange) -> Span,
    joint_with_next: bool,
) {
    let kind = token.kind();
    if kind.is_trivia() {
        return;
    }

    if kind == SyntaxKind::LIFETIME_IDENT {
        push_lifetime(builder, token, span_for_range);
    } else if kind.is_any_identifier() || kind == T![_] {
        builder.push(Leaf::Ident(tt::Ident::new(
            token.text(),
            span_for_range(token.text_range()),
        )));
    } else if kind.is_literal() {
        builder.push(Leaf::Literal(tt::token_to_literal(
            token.text(),
            span_for_range(token.text_range()),
        )));
    } else if kind.is_punct() {
        push_punct_token(builder, token, span_for_range, joint_with_next);
    }
}

fn push_lifetime(
    builder: &mut tt::TopSubtreeBuilder,
    token: &SyntaxToken,
    span_for_range: &mut dyn FnMut(TextRange) -> Span,
) {
    let range = token.text_range();
    let apostrophe = TextRange::at(range.start(), TextSize::of('\''));
    builder.push(Leaf::Punct(tt::Punct {
        char: '\'',
        spacing: Spacing::Joint,
        span: span_for_range(apostrophe),
    }));
    builder.push(Leaf::Ident(tt::Ident {
        sym: Symbol::new(token.text().trim_start_matches('\'')),
        span: span_for_range(TextRange::new(
            range.start() + TextSize::of('\''),
            range.end(),
        )),
        is_raw: tt::IdentIsRaw::No,
    }));
}

fn push_punct_token(
    builder: &mut tt::TopSubtreeBuilder,
    token: &SyntaxToken,
    span_for_range: &mut dyn FnMut(TextRange) -> Span,
    joint_with_next: bool,
) {
    let mut offset = token.text_range().start();
    let chars = token.text().chars().collect::<Vec<_>>();
    for (index, char) in chars.iter().copied().enumerate() {
        let len = TextSize::of(char);
        let spacing = if index + 1 != chars.len() || joint_with_next {
            Spacing::Joint
        } else {
            Spacing::Alone
        };
        builder.push(Leaf::Punct(tt::Punct {
            char,
            spacing,
            span: span_for_range(TextRange::at(offset, len)),
        }));
        offset += len;
    }
}

#[cfg(test)]
mod tests {
    use rg_syntax::TextSize;

    use super::*;

    #[test]
    fn span_for_range_in_file_filters_mapped_file() {
        let span = SpanFactory::new(7, Edition::CURRENT)
            .span_for(TextRange::new(TextSize::new(3), TextSize::new(8)));
        let span_map = ExpansionSpanMap {
            spans: vec![(TextSize::new(5), span)],
        };
        let generated_range = TextRange::new(TextSize::new(0), TextSize::new(5));

        assert_eq!(
            span_map.span_for_range_in_file(generated_range, 7),
            Some(span),
        );
        assert_eq!(span_map.span_for_range_in_file(generated_range, 8), None);
    }
}
