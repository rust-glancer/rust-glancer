//! Syntax helpers shared by the function-body lowering modules.

use ra_syntax::{
    AstNode as _,
    ast::{self, PathSegmentKind},
};

use rg_def_map::{Path, PathSegment};
use rg_parse::{FileId, Span};
use rg_text::Name;

use crate::ir::{BodyPath, BodySource, LiteralKind};

use super::function::FunctionBodyLowering;

impl LiteralKind {
    pub(super) fn from_ast(literal: &ast::Literal) -> Self {
        match literal.kind() {
            ast::LiteralKind::Bool(_) => Self::Bool,
            ast::LiteralKind::Char(_) => Self::Char,
            ast::LiteralKind::Byte(_) => Self::Int,
            ast::LiteralKind::FloatNumber(_) => Self::Float,
            ast::LiteralKind::IntNumber(_) => Self::Int,
            ast::LiteralKind::String(_)
            | ast::LiteralKind::ByteString(_)
            | ast::LiteralKind::CString(_) => Self::String,
        }
    }
}

impl FunctionBodyLowering<'_> {
    pub(super) fn lower_body_path(&mut self, path: ast::Path) -> Option<BodyPath> {
        let source_span = Span::from_text_range(path.syntax().text_range());
        let absolute = path
            .first_segment()
            .is_some_and(|segment| segment.coloncolon_token().is_some());
        let mut segments = Vec::new();
        let mut segment_spans = Vec::new();
        self.collect_path_segments(&path, &mut segments, &mut segment_spans)?;

        if segments.is_empty() {
            return None;
        }

        Some(BodyPath::new(
            source_span,
            Path { absolute, segments },
            segment_spans,
        ))
    }

    pub(super) fn intern_ast_name(&mut self, name: ast::Name) -> Name {
        self.intern_name_text(name.text())
    }

    pub(super) fn intern_ast_name_ref(&mut self, name_ref: ast::NameRef) -> Name {
        self.intern_name_text(name_ref.text())
    }

    pub(super) fn intern_ast_name_or_name_ref(&mut self, name: ast::NameOrNameRef) -> Name {
        self.intern_name_text(name.text())
    }

    fn collect_path_segments(
        &mut self,
        path: &ast::Path,
        segments: &mut Vec<PathSegment>,
        segment_spans: &mut Vec<Span>,
    ) -> Option<()> {
        if let Some(qualifier) = path.qualifier() {
            self.collect_path_segments(&qualifier, segments, segment_spans)?;
        }

        if let Some(segment) = path.segment() {
            let (segment, span) = self.lower_path_segment(segment)?;
            segments.push(segment);
            segment_spans.push(span);
        }

        Some(())
    }

    fn lower_path_segment(&mut self, segment: ast::PathSegment) -> Option<(PathSegment, Span)> {
        match segment.kind()? {
            PathSegmentKind::Name(name_ref) => {
                let span = self.source(name_ref.syntax()).span;
                Some((PathSegment::Name(self.intern_ast_name_ref(name_ref)), span))
            }
            // Body paths share DefMap's compact path shape. `Self` behaves like a normal type
            // name there, while value/module path keywords keep their semantic segment kind.
            PathSegmentKind::SelfTypeKw => {
                let token = segment.self_type_token()?;
                Some((
                    PathSegment::Name(self.intern_name_text(token.text())),
                    Span::from_text_range(token.text_range()),
                ))
            }
            PathSegmentKind::SelfKw => {
                let token = segment.self_token()?;
                Some((
                    PathSegment::SelfKw,
                    Span::from_text_range(token.text_range()),
                ))
            }
            PathSegmentKind::SuperKw => {
                let token = segment.super_token()?;
                Some((
                    PathSegment::SuperKw,
                    Span::from_text_range(token.text_range()),
                ))
            }
            PathSegmentKind::CrateKw => {
                let token = segment.crate_token()?;
                Some((
                    PathSegment::CrateKw,
                    Span::from_text_range(token.text_range()),
                ))
            }
            // Type-qualified segments such as `<T as Trait>::Assoc` need a richer path model
            // than Body IR has. Treat the whole path as unsupported instead of inventing a fake
            // textual segment that could accidentally participate in normal name resolution.
            PathSegmentKind::Type { .. } => None,
        }
    }

    fn intern_name_text(&mut self, text: impl AsRef<str>) -> Name {
        let text = text.as_ref();
        self.interner
            .intern(text.strip_prefix("r#").unwrap_or(text))
    }
}

pub(super) fn source_for(file_id: FileId, syntax: &ra_syntax::SyntaxNode) -> BodySource {
    BodySource {
        file_id,
        span: Span::from_text_range(syntax.text_range()),
    }
}

#[cfg(test)]
mod tests {
    use ra_syntax::{AstNode as _, Edition, SourceFile, ast};

    use crate::ir::LiteralKind;

    #[test]
    fn classifies_rust_literal_tokens() {
        let cases = [
            ("true", LiteralKind::Bool, "bool literal"),
            ("'x'", LiteralKind::Char, "char literal"),
            ("b'x'", LiteralKind::Int, "byte literal"),
            ("42", LiteralKind::Int, "integer literal"),
            ("1.5", LiteralKind::Float, "decimal float literal"),
            ("1e10", LiteralKind::Float, "exponent float literal"),
            (
                "1E-10",
                LiteralKind::Float,
                "negative exponent float literal",
            ),
            (r#""text""#, LiteralKind::String, "string literal"),
            (r##"r#"text"#"##, LiteralKind::String, "raw string literal"),
            (r#"b"text""#, LiteralKind::String, "byte string literal"),
            (
                r##"br#"text"#"##,
                LiteralKind::String,
                "raw byte string literal",
            ),
        ];

        for (expr, expected, label) in cases {
            let source = format!("fn main() {{ let _ = {expr}; }}");
            let file = SourceFile::parse(&source, Edition::CURRENT)
                .ok()
                .expect("literal fixture should parse");
            let literal = file
                .syntax()
                .descendants()
                .find_map(ast::Literal::cast)
                .expect("fixture should contain a literal expression");

            assert_eq!(LiteralKind::from_ast(&literal), expected, "{label}");
        }
    }
}
