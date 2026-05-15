//! Syntax helpers shared by the function-body lowering modules.

use ra_syntax::{AstNode as _, ast};

use rg_def_map::{Path, PathSegment};
use rg_parse::{FileId, Span};
use rg_text::NameInterner;

use crate::ir::{BodyPath, BodySource, LiteralKind};

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

pub(super) fn body_path_from_ast(path: ast::Path, interner: &mut NameInterner) -> BodyPath {
    let source_span = Span::from_text_range(path.syntax().text_range());
    let absolute = path
        .first_segment()
        .is_some_and(|segment| segment.coloncolon_token().is_some());
    let mut segments = Vec::new();
    let mut segment_spans = Vec::new();
    collect_path_segments(&path, interner, &mut segments, &mut segment_spans);

    BodyPath::new(source_span, Path { absolute, segments }, segment_spans)
}

fn collect_path_segments(
    path: &ast::Path,
    interner: &mut NameInterner,
    segments: &mut Vec<PathSegment>,
    segment_spans: &mut Vec<Span>,
) {
    if let Some(qualifier) = path.qualifier() {
        collect_path_segments(&qualifier, interner, segments, segment_spans);
    }

    if let Some(segment) = path.segment() {
        let Some(name_ref) = segment.name_ref() else {
            segments.push(PathSegment::Name(
                interner.intern(normalized_syntax(&segment)),
            ));
            segment_spans.push(Span::from_text_range(segment.syntax().text_range()));
            return;
        };
        let name = name_ref.syntax().text().to_string();
        segment_spans.push(Span::from_text_range(name_ref.syntax().text_range()));

        segments.push(match name.as_str() {
            "self" => PathSegment::SelfKw,
            "super" => PathSegment::SuperKw,
            "crate" => PathSegment::CrateKw,
            name => PathSegment::Name(interner.intern(name)),
        });
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
