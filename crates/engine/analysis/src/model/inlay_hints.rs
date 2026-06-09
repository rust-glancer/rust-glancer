use rg_parse::{FileId, Span};

/// One best-effort editor annotation anchored to a source span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlayHint {
    pub file_id: FileId,
    pub span: Span,
    pub position: InlayHintPosition,
    pub kind: InlayHintKind,
    pub label: String,
    pub padding_left: Option<bool>,
    pub padding_right: Option<bool>,
}

impl InlayHint {
    /// Turns the source span plus before/after preference into the insertion-side offset.
    pub fn text_offset(&self) -> u32 {
        match self.position {
            InlayHintPosition::Before => self.span.text.start,
            InlayHintPosition::After => self.span.text.end,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlayHintPosition {
    Before,
    After,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlayHintKind {
    Type,
    Parameter,
    /// Local fallback for hints that do not fit LSP's type/parameter buckets.
    Text,
}
