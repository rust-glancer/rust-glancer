//! Ordinary request-source syntax shared by code-action providers.

use rg_parse::{Span, TextSpan};
use rg_syntax::{AstNode, SourceFile, TextSize, algo::find_node_at_offset, ast};

/// The exact editor text, its ordinary Rust parse, and the requested part of the file.
///
/// Code actions inspect complete syntax that is already present in the buffer, unlike completion,
/// which injects a marker before parsing incomplete text. Keeping this ordinary tree together with
/// the request selection gives every provider the same rules for choosing a node.
pub(super) struct CodeActionSyntax<'source> {
    source: &'source str,
    file: SourceFile,
    selection: RequestSelection,
}

impl<'source> CodeActionSyntax<'source> {
    pub(super) fn new(source: &'source str, file: SourceFile, range: TextSpan) -> Self {
        Self {
            source,
            file,
            selection: RequestSelection::from(range),
        }
    }

    pub(super) fn source(&self) -> &'source str {
        self.source
    }

    pub(super) fn file(&self) -> &SourceFile {
        &self.file
    }

    /// Find a node under the byte where the request begins.
    ///
    /// This is useful for actions whose owner is determined only by the cursor or selection start,
    /// such as finding the `impl` that will receive generated members.
    pub(super) fn node_at_start<N: AstNode>(&self) -> Option<N> {
        find_node_at_offset(self.file.syntax(), TextSize::from(self.selection.start()))
    }

    /// Find a node touched by the cursor or overlapped by the selection.
    ///
    /// An editor selection can end on the intended token even when its first byte is in nearby
    /// whitespace, so non-empty selections also try their end. The final applicability check
    /// rejects a neighboring node returned for a token boundary.
    fn node_at_request<N: AstNode>(&self) -> Option<N> {
        let node = self.node_at_start().or_else(|| {
            self.selection
                .end()
                .and_then(|end| find_node_at_offset(self.file.syntax(), TextSize::from(end)))
        })?;
        self.request_applies_to(&node).then_some(node)
    }

    /// Return a path name under the request start without applying selection-end fallback.
    pub(super) fn path_name_at_start(&self) -> Option<PathNameSyntax> {
        PathNameSyntax::from_name(self.node_at_start()?)
    }

    /// Return a path name touched by the cursor or overlapped by the selection.
    pub(super) fn path_name_at_request(&self) -> Option<PathNameSyntax> {
        PathNameSyntax::from_name(self.node_at_request()?)
    }

    /// Check whether this request identifies a node.
    ///
    /// A cursor may sit immediately after a token and still refer to it. A non-empty selection must
    /// overlap the node; merely touching its closing edge is not enough.
    pub(super) fn request_applies_to<N: AstNode>(&self, node: &N) -> bool {
        self.selection
            .applies_to(Span::from_text_range(node.syntax().text_range()).text)
    }

    /// Check whether the first byte of the request lies in a node or at its closing edge.
    pub(super) fn request_starts_on<N: AstNode>(&self, node: &N) -> bool {
        Span::from_text_range(node.syntax().text_range())
            .text
            .touches(self.selection.start())
    }
}

/// The path nodes represented by one identifier in source.
///
/// For the selected `User` in `crate::models::User`, `name` is the `User` token, `segment` also
/// owns any arguments such as `<T>`, and `path` is the complete `crate::models::User` path. A
/// `NameRef` used in another grammar shape has no `PathNameSyntax`.
pub(super) struct PathNameSyntax {
    name: ast::NameRef,
    segment: ast::PathSegment,
    path: ast::Path,
}

impl PathNameSyntax {
    fn from_name(name: ast::NameRef) -> Option<Self> {
        let segment = name.syntax().parent().and_then(ast::PathSegment::cast)?;
        let path = segment.parent_path();
        Some(Self {
            name,
            segment,
            path,
        })
    }

    pub(super) fn name(&self) -> &ast::NameRef {
        &self.name
    }

    pub(super) fn segment(&self) -> &ast::PathSegment {
        &self.segment
    }

    pub(super) fn path(&self) -> &ast::Path {
        &self.path
    }
}

/// Whether the editor identified source with a cursor or a non-empty selection.
///
/// The distinction matters at token boundaries: a cursor is allowed to touch a token's end, while
/// a selection must share at least one byte with the token.
enum RequestSelection {
    Cursor(u32),
    Range(TextSpan),
}

impl RequestSelection {
    fn start(&self) -> u32 {
        match self {
            Self::Cursor(offset) => *offset,
            Self::Range(range) => range.start,
        }
    }

    fn end(&self) -> Option<u32> {
        match self {
            Self::Cursor(_) => None,
            Self::Range(range) => Some(range.end),
        }
    }

    fn applies_to(&self, span: TextSpan) -> bool {
        match self {
            Self::Cursor(offset) => span.touches(*offset),
            Self::Range(range) => span.start < range.end && range.start < span.end,
        }
    }
}

impl From<TextSpan> for RequestSelection {
    fn from(range: TextSpan) -> Self {
        if range.is_empty() {
            Self::Cursor(range.start)
        } else {
            Self::Range(range)
        }
    }
}
