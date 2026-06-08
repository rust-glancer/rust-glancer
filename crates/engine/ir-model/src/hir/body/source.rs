use rg_std::MemorySize;
use wincode::{SchemaRead, SchemaWrite};

use rg_parse::{FileId, Span};

/// Source location attached to every body node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct BodySource {
    pub file_id: FileId,
    pub span: Span,
}
