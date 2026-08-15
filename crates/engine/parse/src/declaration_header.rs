//! Finds the saved declaration that has the same header as one in current editor text.
//!
//! A declaration can move when text before it changes, so its saved byte offset is not a stable
//! way to find it. Its header and the headers around it remain useful, however: `fn load()` inside
//! `impl Store` can still be found after its body changes or the whole impl moves. Whitespace and
//! comments do not take part in this comparison. New, renamed, or duplicate declarations do not
//! match anything because the saved declaration would not be known with certainty.

use std::collections::HashMap;

use rg_std::ExpectedUnique;
use rg_syntax::{AstNode as _, SourceFile, SyntaxKind, SyntaxNode, SyntaxToken, TextSize, ast};

use crate::Span;

/// One declaration header together with the declaration headers around it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DeclarationHeaderAnchor {
    declaration: HeaderFingerprint,
    containers: Vec<HeaderFingerprint>,
}

impl DeclarationHeaderAnchor {
    /// Describe one declaration without using its source offset or body text.
    pub fn for_node(node: &SyntaxNode) -> Option<Self> {
        let declaration = HeaderFingerprint::for_node(node)?;
        let mut containers = node
            .ancestors()
            .skip(1)
            .filter_map(|ancestor| HeaderFingerprint::for_node(&ancestor))
            .collect::<Vec<_>>();
        containers.reverse();
        Some(Self {
            declaration,
            containers,
        })
    }
}

/// Unique declaration matches between one current and one saved syntax tree.
///
/// Matching only by header is deliberately conservative. An anchor is kept only when it occurs
/// once on each side, so two current declarations can never borrow the same saved identity. The
/// index also remembers corresponding header-token ranges for hover and navigation without
/// treating current and saved byte offsets as interchangeable.
#[derive(Debug, Clone)]
pub struct DeclarationAssociationIndex {
    associations: Vec<DeclarationAssociation>,
    current_associations: HashMap<Span, ExpectedUnique<usize>>,
    saved_header_tokens: HashMap<Span, (usize, usize)>,
}

impl DeclarationAssociationIndex {
    /// Build every unambiguous current-to-saved declaration pairing for one source interpretation.
    pub fn new(current: &SourceFile, saved: &SourceFile) -> Self {
        let current = DeclarationShape::collect(current);
        let saved = DeclarationShape::collect(saved);
        let mut associations = Vec::new();
        let mut current_associations = HashMap::new();
        let mut saved_header_tokens = HashMap::new();

        for (anchor, current_shapes) in current {
            let Some(saved_shapes) = saved.get(&anchor) else {
                continue;
            };
            let ([current], [saved]) = (current_shapes.as_slice(), saved_shapes.as_slice()) else {
                for current in current_shapes {
                    current_associations.insert(current.span, ExpectedUnique::Ambiguous);
                }
                continue;
            };
            let association = associations.len();
            associations.push(DeclarationAssociation {
                current: current.clone(),
                saved: saved.clone(),
            });
            current_associations.insert(current.span, ExpectedUnique::One(association));
            for (token, span) in saved.header_tokens.iter().enumerate() {
                saved_header_tokens.insert(*span, (association, token));
            }
        }

        Self {
            associations,
            current_associations,
            saved_header_tokens,
        }
    }

    /// Find the saved declaration with the same header and containing declarations.
    pub fn saved_declaration_for_current(&self, current: &SyntaxNode) -> ExpectedUnique<Span> {
        let current = Span::from_text_range(current.text_range());
        self.current_associations
            .get(&current)
            .cloned()
            .unwrap_or_default()
            .map(|association| self.associations[association].saved.span)
    }

    /// Return the saved declaration paired with this exact current declaration span.
    pub fn saved_declaration_for_current_span(&self, current: Span) -> Option<Span> {
        let association = *self.current_associations.get(&current)?.as_option()?;
        Some(self.associations.get(association)?.saved.span)
    }

    /// Map a token selected in the current header to the corresponding saved header token.
    pub fn saved_header_span(&self, cursor: &DeclarationHeaderCursor) -> Option<Span> {
        let association = *self
            .current_associations
            .get(&cursor.current_declaration_span)?
            .as_option()?;
        self.associations
            .get(association)?
            .saved
            .header_tokens
            .get(cursor.token_index)
            .copied()
    }

    /// Map a saved declaration-name or header-token range into current source.
    pub fn current_header_span_for_saved(&self, saved: Span) -> Option<Span> {
        let &(association, token) = self.saved_header_tokens.get(&saved)?;
        self.associations
            .get(association)?
            .current
            .header_tokens
            .get(token)
            .copied()
    }
}

#[derive(Debug, Clone)]
struct DeclarationAssociation {
    current: DeclarationShape,
    saved: DeclarationShape,
}

#[derive(Debug, Clone)]
struct DeclarationShape {
    span: Span,
    header_tokens: Vec<Span>,
}

impl DeclarationShape {
    fn collect(file: &SourceFile) -> HashMap<DeclarationHeaderAnchor, Vec<Self>> {
        let mut declarations = HashMap::<DeclarationHeaderAnchor, Vec<Self>>::new();
        for node in file.syntax().descendants() {
            let Some(anchor) = DeclarationHeaderAnchor::for_node(&node) else {
                continue;
            };
            let header_tokens = HeaderFingerprint::tokens(&node)
                .expect("a declaration anchor should have header tokens")
                .into_iter()
                .map(|token| Span::from_text_range(token.text_range()))
                .collect();
            declarations.entry(anchor).or_default().push(Self {
                span: Span::from_text_range(node.text_range()),
                header_tokens,
            });
        }
        declarations
    }
}

/// A token in a current declaration header that may have the same token in saved source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclarationHeaderCursor {
    current_declaration_span: Span,
    token_index: usize,
    current_span: Span,
    header_len: usize,
}

impl DeclarationHeaderCursor {
    /// Find a non-trivia declaration-header token under the cursor.
    pub fn at(file: &SourceFile, offset: u32) -> Option<Self> {
        let offset = TextSize::from(offset);
        file.syntax()
            .token_at_offset(offset)
            .filter(|token| !token.kind().is_trivia())
            .filter_map(Self::for_token)
            .min_by_key(|cursor| cursor.header_len)
    }

    /// Exact range of the token in current source.
    pub fn current_span(&self) -> Span {
        self.current_span
    }

    fn for_token(token: SyntaxToken) -> Option<Self> {
        token.parent_ancestors().find_map(|node| {
            let tokens = HeaderFingerprint::tokens(&node)?;
            let token_index = tokens.iter().position(|candidate| *candidate == token)?;
            Some(Self {
                current_declaration_span: Span::from_text_range(node.text_range()),
                token_index,
                current_span: Span::from_text_range(token.text_range()),
                header_len: tokens.len(),
            })
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct HeaderFingerprint {
    kind: SyntaxKind,
    tokens: Vec<HeaderToken>,
}

impl HeaderFingerprint {
    fn for_node(node: &SyntaxNode) -> Option<Self> {
        Some(Self {
            kind: node.kind(),
            tokens: Self::tokens(node)?
                .into_iter()
                .map(HeaderToken::from)
                .collect(),
        })
    }

    fn tokens(node: &SyntaxNode) -> Option<Vec<SyntaxToken>> {
        let boundary = Self::header_boundary(node)?;
        Some(
            node.descendants_with_tokens()
                .filter_map(|element| element.into_token())
                .filter(|token| token.text_range().end() <= boundary && !token.kind().is_trivia())
                .collect(),
        )
    }

    /// Return the first token that belongs to a declaration's body or child list.
    ///
    /// Everything before this boundary describes the saved identity. Declarations without a body
    /// or child list use their complete syntax, which covers forms such as `type Output = Value;`
    /// and trait methods ending in `;`.
    fn header_boundary(node: &SyntaxNode) -> Option<TextSize> {
        if let Some(item) = ast::Fn::cast(node.clone()) {
            return Some(
                item.body()
                    .map(|body| body.syntax().text_range().start())
                    .unwrap_or_else(|| node.text_range().end()),
            );
        }
        if let Some(item) = ast::Const::cast(node.clone()) {
            return Some(
                item.body()
                    .map(|body| body.syntax().text_range().start())
                    .unwrap_or_else(|| node.text_range().end()),
            );
        }
        if let Some(item) = ast::Static::cast(node.clone()) {
            return Some(
                item.body()
                    .map(|body| body.syntax().text_range().start())
                    .unwrap_or_else(|| node.text_range().end()),
            );
        }
        if let Some(item) = ast::Module::cast(node.clone()) {
            return Some(
                item.item_list()
                    .map(|list| list.syntax().text_range().start())
                    .unwrap_or_else(|| node.text_range().end()),
            );
        }
        if let Some(item) = ast::Trait::cast(node.clone()) {
            return Some(
                item.assoc_item_list()
                    .map(|list| list.syntax().text_range().start())
                    .unwrap_or_else(|| node.text_range().end()),
            );
        }
        if let Some(item) = ast::Impl::cast(node.clone()) {
            return Some(
                item.assoc_item_list()
                    .map(|list| list.syntax().text_range().start())
                    .unwrap_or_else(|| node.text_range().end()),
            );
        }
        if let Some(item) = ast::Struct::cast(node.clone()) {
            return Some(
                item.field_list()
                    .map(|list| list.syntax().text_range().start())
                    .unwrap_or_else(|| node.text_range().end()),
            );
        }
        if let Some(item) = ast::Enum::cast(node.clone()) {
            return Some(
                item.variant_list()
                    .map(|list| list.syntax().text_range().start())
                    .unwrap_or_else(|| node.text_range().end()),
            );
        }
        if let Some(item) = ast::Union::cast(node.clone()) {
            return Some(
                item.record_field_list()
                    .map(|list| list.syntax().text_range().start())
                    .unwrap_or_else(|| node.text_range().end()),
            );
        }
        if let Some(item) = ast::Variant::cast(node.clone()) {
            return Some(
                item.field_list()
                    .map(|list| list.syntax().text_range().start())
                    .unwrap_or_else(|| node.text_range().end()),
            );
        }
        if ast::TypeAlias::can_cast(node.kind())
            || ast::RecordField::can_cast(node.kind())
            || ast::TupleField::can_cast(node.kind())
        {
            return Some(node.text_range().end());
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct HeaderToken {
    kind: SyntaxKind,
    text: String,
}

impl From<SyntaxToken> for HeaderToken {
    fn from(token: SyntaxToken) -> Self {
        Self {
            kind: token.kind(),
            text: token.text().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use rg_syntax::{Edition, SourceFile};

    use super::{DeclarationAssociationIndex, DeclarationHeaderCursor};

    #[test]
    fn maps_moved_headers_without_using_body_text_or_offsets() {
        let saved = r#"
struct Foo;

impl Foo {
    fn new() {
        let saved = true;
    }
}
"#;
        let current = r#"
use std::fmt;

struct Foo;

impl Foo {
    fn new() {
        let current = true;
    }
}
"#;
        let saved_file = SourceFile::parse(saved, Edition::Edition2024).tree();
        let current_file = SourceFile::parse(current, Edition::Edition2024).tree();
        let associations = DeclarationAssociationIndex::new(&current_file, &saved_file);

        for (header, name) in [("impl Foo", "Foo"), ("fn new", "new")] {
            let token_offset = current
                .find(header)
                .expect("current fixture should contain header token")
                + header
                    .find(name)
                    .expect("header fixture should contain its declaration name");
            let cursor = DeclarationHeaderCursor::at(
                &current_file,
                u32::try_from(token_offset).expect("header offset should fit u32"),
            )
            .expect("current header should be recognized");
            let saved_span = associations
                .saved_header_span(&cursor)
                .expect("current header should match saved syntax");
            assert_eq!(
                &saved[usize::try_from(saved_span.text.start).expect("span should fit usize")
                    ..usize::try_from(saved_span.text.end).expect("span should fit usize")],
                name,
            );
        }
    }

    #[test]
    fn rejects_changed_and_ambiguous_headers() {
        let current = SourceFile::parse(
            "struct Foo; impl Foo { fn changed() {} }",
            Edition::Edition2024,
        )
        .tree();
        let changed_offset = u32::try_from("struct Foo; impl Foo { fn ".len())
            .expect("header offset should fit u32");
        let cursor = DeclarationHeaderCursor::at(&current, changed_offset)
            .expect("changed method header should be recognized");

        let renamed = SourceFile::parse(
            "struct Foo; impl Foo { fn saved() {} }",
            Edition::Edition2024,
        )
        .tree();
        let renamed_associations = DeclarationAssociationIndex::new(&current, &renamed);
        assert_eq!(renamed_associations.saved_header_span(&cursor), None);

        let duplicate = SourceFile::parse(
            "struct Foo; impl Foo { fn changed() {} } impl Foo { fn changed() {} }",
            Edition::Edition2024,
        )
        .tree();
        let duplicate_associations = DeclarationAssociationIndex::new(&current, &duplicate);
        assert_eq!(duplicate_associations.saved_header_span(&cursor), None);
    }

    #[test]
    fn rejects_duplicate_current_declarations() {
        let current = SourceFile::parse(
            "struct Foo; impl Foo { fn load() {} fn load() {} }",
            Edition::Edition2024,
        )
        .tree();
        let saved = SourceFile::parse(
            "struct Foo; impl Foo { fn load() {} }",
            Edition::Edition2024,
        )
        .tree();
        let associations = DeclarationAssociationIndex::new(&current, &saved);

        for offset in [
            "struct Foo; impl Foo { fn ".len(),
            "struct Foo; impl Foo { fn load() {} fn ".len(),
        ] {
            let cursor = DeclarationHeaderCursor::at(
                &current,
                u32::try_from(offset).expect("fixture offset should fit u32"),
            )
            .expect("function header should be recognized");
            assert_eq!(associations.saved_header_span(&cursor), None);
        }
    }
}
