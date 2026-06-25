//! Typed query model used by fixture vectors and execution.

/// One LSP request that should be sent to both compared servers.
///
/// The case stores fixture-relative paths because result normalization also compares
/// fixture-relative locations. Positions are LSP coordinates, not byte offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QueryCase {
    label: &'static str,
    source_path: &'static str,
    position: SourcePosition,
    kind: QueryKind,
}

impl QueryCase {
    pub(super) const fn new(
        label: &'static str,
        source_path: &'static str,
        position: SourcePosition,
        kind: QueryKind,
    ) -> Self {
        Self {
            label,
            source_path,
            position,
            kind,
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        self.label
    }

    pub(crate) fn source_path(&self) -> &'static str {
        self.source_path
    }

    pub(crate) fn position(&self) -> SourcePosition {
        self.position
    }

    pub(crate) fn kind(&self) -> QueryKind {
        self.kind
    }
}

/// Zero-based position in the same UTF-16 coordinate space used by LSP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourcePosition {
    /// Zero-based UTF-16 line used by LSP positions.
    line: u32,
    /// Zero-based UTF-16 character offset used by LSP positions.
    character: u32,
}

impl SourcePosition {
    pub(super) const fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }

    pub(crate) fn line(self) -> u32 {
        self.line
    }

    pub(crate) fn character(self) -> u32 {
        self.character
    }

    pub(crate) fn to_lsp(self) -> ls_types::Position {
        ls_types::Position::new(self.line, self.character)
    }
}

/// LSP request family plus method-specific options needed for comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueryKind {
    References { include_declaration: bool },
    GotoDefinition,
    Hover,
}

impl QueryKind {
    pub(crate) fn lsp_method(self) -> &'static str {
        use ls_types::{request, request::Request as _};

        match self {
            Self::References { .. } => request::References::METHOD,
            Self::GotoDefinition => request::GotoDefinition::METHOD,
            Self::Hover => request::HoverRequest::METHOD,
        }
    }

    pub(crate) fn is_references(self) -> bool {
        matches!(self, Self::References { .. })
    }

    pub(crate) fn references_include_declaration(self) -> Option<bool> {
        match self {
            Self::References {
                include_declaration,
            } => Some(include_declaration),
            Self::GotoDefinition | Self::Hover => None,
        }
    }

    pub(crate) fn is_goto_definition(self) -> bool {
        matches!(self, Self::GotoDefinition)
    }

    pub(crate) fn is_hover(self) -> bool {
        matches!(self, Self::Hover)
    }
}
