use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use rg_parse::{FileId, Span};

/// Whether a body source span belongs to the user's syntax or to lowered macro output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
enum BodySourceKind {
    /// Syntax written directly in a source file.
    Written,
    /// Syntax produced by expanding a macro while lowering a body.
    MacroGenerated,
}

/// Source location attached to every body node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodySource {
    pub file_id: FileId,
    pub span: Span,
    kind: BodySourceKind,
}

impl BodySource {
    pub fn written(file_id: FileId, span: Span) -> Self {
        Self {
            file_id,
            span,
            kind: BodySourceKind::Written,
        }
    }

    pub fn macro_generated(file_id: FileId, span: Span) -> Self {
        Self {
            file_id,
            span,
            kind: BodySourceKind::MacroGenerated,
        }
    }

    pub fn is_written(self) -> bool {
        matches!(self.kind, BodySourceKind::Written)
    }

    /// Returns true when this span belongs to user-written body syntax in one concrete file.
    pub fn is_written_in_file(self, file_id: FileId) -> bool {
        self.file_id == file_id && self.is_written()
    }

    /// Returns true when this span is user-written and passes an optional file filter.
    pub fn is_written_in_selected_file(self, file_id: Option<FileId>) -> bool {
        file_id.is_none_or(|file_id| self.file_id == file_id) && self.is_written()
    }
}
