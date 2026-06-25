//! Typed query model used by fixture vectors and execution.

/// One LSP request that should be sent to both compared servers.
///
/// The case stores fixture-relative paths because result normalization also compares
/// fixture-relative locations. Positions are LSP coordinates, not byte offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QueryCase {
    label: &'static str,
    kind: QueryKind,
    target: QueryTarget,
}

impl QueryCase {
    pub(super) const fn position(
        label: &'static str,
        kind: QueryKind,
        source_path: &'static str,
        position: SourcePosition,
    ) -> Self {
        Self {
            label,
            kind,
            target: QueryTarget::Position {
                source_path,
                position,
            },
        }
    }

    pub(super) const fn file(
        label: &'static str,
        kind: QueryKind,
        source_path: &'static str,
    ) -> Self {
        Self {
            label,
            kind,
            target: QueryTarget::File { source_path },
        }
    }

    pub(super) const fn workspace_query(
        label: &'static str,
        kind: QueryKind,
        query: &'static str,
    ) -> Self {
        Self {
            label,
            kind,
            target: QueryTarget::Workspace { query },
        }
    }

    pub(super) const fn rename(
        label: &'static str,
        kind: QueryKind,
        source_path: &'static str,
        position: SourcePosition,
        new_name: &'static str,
    ) -> Self {
        Self {
            label,
            kind,
            target: QueryTarget::Rename {
                source_path,
                position,
                new_name,
            },
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        self.label
    }

    pub(crate) fn kind(&self) -> QueryKind {
        self.kind
    }

    pub(crate) fn target(&self) -> QueryTarget {
        self.target
    }

    pub(crate) fn source_path(&self) -> Option<&'static str> {
        match self.target {
            QueryTarget::Position { source_path, .. }
            | QueryTarget::File { source_path }
            | QueryTarget::Rename { source_path, .. } => Some(source_path),
            QueryTarget::Workspace { .. } => None,
        }
    }
}

/// Request input shape for one query case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueryTarget {
    Position {
        source_path: &'static str,
        position: SourcePosition,
    },
    File {
        source_path: &'static str,
    },
    Workspace {
        query: &'static str,
    },
    Rename {
        source_path: &'static str,
        position: SourcePosition,
        new_name: &'static str,
    },
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
    TypeDefinition,
    Implementation,
    PrepareRename,
    Rename,
    DocumentHighlight,
    DocumentSymbol,
    WorkspaceSymbol,
    InlayHint,
    Hover,
}

impl QueryKind {
    pub(crate) fn lsp_method(self) -> &'static str {
        use ls_types::{request, request::Request as _};

        match self {
            Self::References { .. } => request::References::METHOD,
            Self::GotoDefinition => request::GotoDefinition::METHOD,
            Self::TypeDefinition => request::GotoTypeDefinition::METHOD,
            Self::Implementation => request::GotoImplementation::METHOD,
            Self::PrepareRename => request::PrepareRenameRequest::METHOD,
            Self::Rename => request::Rename::METHOD,
            Self::DocumentHighlight => request::DocumentHighlightRequest::METHOD,
            Self::DocumentSymbol => request::DocumentSymbolRequest::METHOD,
            Self::WorkspaceSymbol => request::WorkspaceSymbolRequest::METHOD,
            Self::InlayHint => request::InlayHintRequest::METHOD,
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
            Self::GotoDefinition
            | Self::TypeDefinition
            | Self::Implementation
            | Self::PrepareRename
            | Self::Rename
            | Self::DocumentHighlight
            | Self::DocumentSymbol
            | Self::WorkspaceSymbol
            | Self::InlayHint
            | Self::Hover => None,
        }
    }

    pub(crate) fn is_goto_definition(self) -> bool {
        matches!(self, Self::GotoDefinition)
    }

    pub(crate) fn is_type_definition(self) -> bool {
        matches!(self, Self::TypeDefinition)
    }

    pub(crate) fn is_implementation(self) -> bool {
        matches!(self, Self::Implementation)
    }

    pub(crate) fn is_prepare_rename(self) -> bool {
        matches!(self, Self::PrepareRename)
    }

    pub(crate) fn is_rename(self) -> bool {
        matches!(self, Self::Rename)
    }

    pub(crate) fn is_document_highlight(self) -> bool {
        matches!(self, Self::DocumentHighlight)
    }

    pub(crate) fn is_document_symbol(self) -> bool {
        matches!(self, Self::DocumentSymbol)
    }

    pub(crate) fn is_workspace_symbol(self) -> bool {
        matches!(self, Self::WorkspaceSymbol)
    }

    pub(crate) fn is_inlay_hint(self) -> bool {
        matches!(self, Self::InlayHint)
    }

    pub(crate) fn is_hover(self) -> bool {
        matches!(self, Self::Hover)
    }
}
