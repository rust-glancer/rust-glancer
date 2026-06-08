//! Symbol projection result models.

use rg_parse::{FileId, Span};

use crate::item::declaration::Declaration;

use super::SymbolKind;

/// One source outline declaration.
///
/// Most nodes come from real declarations, but some syntax-only children, such as tuple variant
/// fields, only exist as outline entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceOutlineDeclaration {
    name: String,
    kind: SymbolKind,
    file_id: FileId,
    span: Span,
    selection_span: Span,
}

impl SourceOutlineDeclaration {
    pub(crate) fn field(file_id: FileId, name: String, span: Span) -> Self {
        Self {
            name,
            kind: SymbolKind::Field,
            file_id,
            span,
            selection_span: span,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn kind(&self) -> SymbolKind {
        self.kind
    }

    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn selection_span(&self) -> Span {
        self.selection_span
    }

    pub fn into_parts(self) -> (String, SymbolKind, FileId, Span, Span) {
        (
            self.name,
            self.kind,
            self.file_id,
            self.span,
            self.selection_span,
        )
    }
}

impl From<Declaration> for SourceOutlineDeclaration {
    fn from(declaration: Declaration) -> Self {
        Self {
            name: declaration.name().to_string(),
            kind: declaration.kind(),
            file_id: declaration.file_id(),
            span: declaration.span(),
            selection_span: declaration.selection_span(),
        }
    }
}

/// Hierarchical source outline node for a single file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceOutlineNode {
    declaration: SourceOutlineDeclaration,
    children: Vec<SourceOutlineNode>,
}

impl SourceOutlineNode {
    pub(crate) fn new(declaration: impl Into<SourceOutlineDeclaration>) -> Self {
        Self {
            declaration: declaration.into(),
            children: Vec::new(),
        }
    }

    pub(crate) fn with_children(mut self, children: Vec<SourceOutlineNode>) -> Self {
        self.children = children;
        self
    }

    pub fn declaration(&self) -> &SourceOutlineDeclaration {
        &self.declaration
    }

    pub fn children(&self) -> &[SourceOutlineNode] {
        &self.children
    }

    pub(crate) fn children_mut(&mut self) -> &mut Vec<SourceOutlineNode> {
        &mut self.children
    }

    pub fn into_parts(self) -> (SourceOutlineDeclaration, Vec<SourceOutlineNode>) {
        (self.declaration, self.children)
    }
}

/// One workspace-wide symbol entry with enough context for search and rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedSymbolEntry {
    declaration: Declaration,
    container_name: Option<String>,
}

impl IndexedSymbolEntry {
    pub(crate) fn new(declaration: Declaration, container_name: Option<String>) -> Self {
        Self {
            declaration,
            container_name,
        }
    }

    pub fn declaration(&self) -> &Declaration {
        &self.declaration
    }

    pub fn container_name(&self) -> Option<&str> {
        self.container_name.as_deref()
    }

    pub fn name(&self) -> &str {
        self.declaration.name()
    }

    pub fn into_parts(self) -> (Declaration, Option<String>) {
        (self.declaration, self.container_name)
    }
}
