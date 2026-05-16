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
    pub(super) fn from_text(text: &str) -> Self {
        if matches!(text, "true" | "false") {
            return Self::Bool;
        }

        if text.starts_with('"') || text.starts_with("r#") || text.starts_with("br#") {
            return Self::String;
        }

        if text.starts_with('\'') {
            return Self::Char;
        }

        if text.contains('.') {
            return Self::Float;
        }

        if text
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_digit())
        {
            return Self::Int;
        }

        Self::Unknown
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

pub(super) fn normalized_syntax(node: &impl ra_syntax::AstNode) -> String {
    normalized_syntax_node(node.syntax())
}

fn normalized_syntax_node(node: &ra_syntax::SyntaxNode) -> String {
    node.text()
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
