//! Extracts documentation while remembering which Rust bytes supplied its text.

use std::ops::Range;

use rg_parse::{TextRangeMap, TextRangeMapping};
use rg_syntax::{
    AstNode as _, AstToken as _, SyntaxNode,
    ast::{self, IsString as _},
};

use super::Documentation;

/// Outer docs (`///`, `#[doc = ...]`) describe the following declaration.
/// Inner docs (`//!`, `#![doc = ...]`) describe the item or file that contains them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentationPlacement {
    Outer,
    Inner,
}

/// Read documentation as Markdown without losing where it came from in the Rust file.
///
/// For `/// [Profile]`, the parser sees `[Profile]`, but a hover cursor still points into the
/// original comment. Joining lines and decoding `#[doc = "..."]` also shifts offsets, so each
/// piece of Markdown remembers the source bytes that supplied it.
///
/// Markdown offsets start at zero in `text`; source offsets start at the beginning of the Rust
/// file. Both count UTF-8 bytes. Hover translates a source cursor into Markdown, while
/// highlighting translates Markdown ranges back into the file. Only the text is kept when
/// this is converted into indexed `Documentation`.
#[derive(Debug, Default)]
pub struct DocumentationSource {
    text: String,
    mappings: TextRangeMap,
}

impl DocumentationSource {
    /// Collect docs attached directly to `node`, without walking into nested declarations.
    ///
    /// For outer docs, pass the declaration's node. For inner docs, pass the node holding its
    /// contents: the source file itself for file docs, or the item list inside `mod api { ... }`.
    /// Choosing that node and placement keeps `//!` comments attached to their containing module.
    pub fn from_node(node: &SyntaxNode, placement: DocumentationPlacement) -> Self {
        let mut docs = Self::default();
        let inner = placement == DocumentationPlacement::Inner;
        // Even an empty comment contributes a separator. Checking the accumulated text would
        // lose leading blank lines, so track whether a fragment was encountered separately.
        let mut has_fragment = false;
        for comment in ast::DocCommentIter::from_syntax_node(node).filter(|comment| {
            if inner {
                comment.is_inner()
            } else {
                comment.is_outer()
            }
        }) {
            let Some((text, prefix)) = comment.doc_comment() else {
                continue;
            };
            if has_fragment {
                docs.text.push('\n');
            }
            has_fragment = true;
            let start = usize::from(comment.syntax().text_range().start() + prefix);
            docs.append_comment(text, start);
        }
        let attributes = node
            .children()
            .filter_map(ast::Attr::cast)
            .filter(|attr| attr.kind().is_inner() == inner)
            .filter_map(|attr| Self::from_attribute(&attr));
        for source in attributes {
            if has_fragment {
                docs.text.push('\n');
            }
            has_fragment = true;
            docs.append_normalized(source);
        }
        docs
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Keep only the Markdown for storage, discarding the temporary source mappings.
    /// Whitespace-only text means there is no documentation to store.
    pub fn into_documentation(self) -> Option<Documentation> {
        Documentation::new(self.text)
    }

    /// Move the Markdown and its mappings into a larger document. After joining the text,
    /// keep this part's starting offset: the map still uses Markdown offsets starting at zero
    /// for this part, and Rust offsets starting at the beginning of its source file.
    pub fn into_parts(self) -> (String, TextRangeMap) {
        (self.text, self.mappings)
    }

    /// Normalize comment text while it can still be borrowed from its syntax token. The bytes
    /// after the doc prefix are copied directly, so each line maps by its original file offset.
    fn append_comment(&mut self, text: &str, source_offset: usize) {
        for (index, range) in Self::normalized_lines(text).enumerate() {
            if index != 0 {
                self.text.push('\n');
            }
            let original = source_offset + range.start..source_offset + range.end;
            self.push(
                &text[range.clone()],
                TextRangeMapping::copied(range, original),
            );
        }
    }

    /// Normalize decoded attribute text through its mappings. Removing padding and joining lines
    /// must keep a decoded character attached to the whole escape that supplied it.
    fn append_normalized(&mut self, source: Self) {
        for (index, range) in Self::normalized_lines(&source.text).enumerate() {
            if index != 0 {
                self.text.push('\n');
            }
            for mapping in source.mappings.project(range) {
                self.push(&source.text[mapping.generated.clone()], mapping);
            }
        }
    }

    /// Remove one optional leading space per line, as in `/// text`. Further spaces remain,
    /// preserving Markdown indentation. Both comments and decoded attributes use these ranges
    /// so their whitespace handling stays the same, including CRLF line endings.
    fn normalized_lines(text: &str) -> impl Iterator<Item = Range<usize>> + '_ {
        let mut offset = 0;
        text.lines().map(move |line| {
            let end = offset + line.len();
            let range = offset + usize::from(line.starts_with(' '))..end;
            offset = end
                + if text[end..].starts_with("\r\n") {
                    2
                } else {
                    1
                };
            range
        })
    }

    /// Move a fragment's generated range to its position in this buffer while preserving its
    /// original spelling and whether it was copied or transformed.
    fn push(&mut self, text: &str, mut mapping: TextRangeMapping) {
        if text.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(text);
        mapping.generated = start..self.text.len();
        self.mappings.push(mapping);
    }

    /// Read a literal doc attribute as Markdown, mapping decoded characters to their Rust spelling.
    /// Raw strings can map their entire contents directly because they do not decode escapes.
    fn from_attribute(attr: &ast::Attr) -> Option<Self> {
        let ast::Meta::KeyValueMeta(meta) = attr.meta()? else {
            return None;
        };
        if meta.path()?.syntax().text() != "doc" {
            return None;
        }
        let ast::Expr::Literal(literal) = meta.expr()? else {
            return None;
        };
        let ast::LiteralKind::String(value) = literal.kind() else {
            return None;
        };
        let mut source = Self::default();
        // Validate the whole literal first. Taking only successful character callbacks could
        // otherwise turn an invalid escape into silently missing text in the Markdown.
        let decoded = value.value().ok()?;
        if value.is_raw() {
            let range = value.text_range_between_quotes()?;
            source.push(
                &decoded,
                TextRangeMapping::copied(
                    0..decoded.len(),
                    usize::from(range.start())..usize::from(range.end()),
                ),
            );
        } else {
            let start = usize::from(value.syntax().text_range().start());
            value.escaped_char_ranges(&mut |range, character| {
                if let Ok(character) = character {
                    let mut buffer = [0; 4];
                    let text = character.encode_utf8(&mut buffer);
                    let original =
                        start + usize::from(range.start())..start + usize::from(range.end());
                    let mapping = if text.len() == original.len() {
                        TextRangeMapping::copied(0..text.len(), original)
                    } else {
                        TextRangeMapping::transformed(0..text.len(), original)
                    };
                    source.push(text, mapping);
                }
            });
        }
        Some(source)
    }
}

#[cfg(test)]
mod tests {
    use super::{DocumentationPlacement, DocumentationSource};
    use rg_syntax::{AstNode as _, Edition, SourceFile, ast};

    #[test]
    fn maps_normalized_comments_and_decoded_attributes() {
        let cases = [
            (
                "///\n///  [`Profile`]\n///\nstruct User;",
                "\n [`Profile`]\n",
                "[`Profile`]",
            ),
            (
                "///\r\n/// café [`Profile`]\r\nstruct User;",
                "\r\ncafé [`Profile`]\r",
                "[`Profile`]",
            ),
            (
                "/** café\n [`Profile`] */ struct User;",
                "café\n[`Profile`] ",
                "[`Profile`]",
            ),
            (
                r##"#[doc = r#"café [`Profile`]"#] struct User;"##,
                "café [`Profile`]",
                "[`Profile`]",
            ),
            (
                r#"#[doc = "café\n [`Pr\u{6f}file`]"] struct User;"#,
                "café\n[`Profile`]",
                r"[`Pr\u{6f}file`]",
            ),
        ];
        for (source, expected, authored) in cases {
            let file = SourceFile::parse(source, Edition::Edition2024).tree();
            let item = file
                .syntax()
                .descendants()
                .find_map(ast::Struct::cast)
                .expect("fixture contains a struct");
            let docs = DocumentationSource::from_node(item.syntax(), DocumentationPlacement::Outer);
            let (markdown, mappings) = docs.into_parts();
            assert_eq!(markdown, expected, "{source}");
            let start = markdown.find("[`").expect("fixture contains a link");
            let end = start + "[`Profile`]".len();
            let ranges = mappings
                .project(start..end)
                .map(|mapping| mapping.original)
                .collect::<Vec<_>>();
            let written = ranges
                .iter()
                .map(|range| &source[range.clone()])
                .collect::<String>();
            assert_eq!(written, authored, "{source}");
            assert_eq!(
                mappings.generated_offset(ranges[0].start),
                Some(start),
                "{source}"
            );
        }
    }
}
