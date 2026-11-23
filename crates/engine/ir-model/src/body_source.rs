use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::{FileId, SourceLocation, SourceLocationKind, Span};

/// Source location shared by body lowering and body-macro expansion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodySource {
    pub file_id: FileId,
    pub span: Span,
    kind: SourceLocationKind,
}

impl BodySource {
    pub fn written(file_id: FileId, span: Span) -> Self {
        Self {
            file_id,
            span,
            kind: SourceLocationKind::Direct,
        }
    }

    pub fn macro_argument(file_id: FileId, span: Span) -> Self {
        Self {
            file_id,
            span,
            kind: SourceLocationKind::MacroArgument,
        }
    }

    pub fn macro_generated(file_id: FileId, span: Span) -> Self {
        Self {
            file_id,
            span,
            kind: SourceLocationKind::NavigationFallback,
        }
    }

    pub fn location(self) -> SourceLocation {
        SourceLocation {
            span: self.span,
            kind: self.kind,
        }
    }

    /// Tests whether the syntax came directly from the file. Name scans can use
    /// `location().is_exact()` to accept copied argument tokens as well.
    pub fn is_written(self) -> bool {
        self.kind == SourceLocationKind::Direct
    }

    /// Returns true when this span belongs to user-written body syntax in one concrete file.
    pub fn is_written_in_file(self, file_id: FileId) -> bool {
        self.file_id == file_id && self.is_written()
    }

    /// Accept direct syntax and exact copies of macro input in the requested file.
    pub fn is_exact_in_file(self, file_id: FileId) -> bool {
        self.file_id == file_id && self.location().is_exact()
    }

    /// Returns true when this span is user-written and passes an optional file filter.
    pub fn is_written_in_selected_file(self, file_id: Option<FileId>) -> bool {
        file_id.is_none_or(|file_id| self.file_id == file_id) && self.is_written()
    }
}
