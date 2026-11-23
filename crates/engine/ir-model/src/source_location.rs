use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::Span;

/// A source occurrence after resolving it to a physical file range in the selected snapshot.
/// The caller still supplies the file identity; this value only describes the range within it.
///
/// One macro invocation can provide two different kinds of location.
///
/// ```text
/// macro_rules! make { ($name:ident) => { struct $name; struct Generated; }; }
/// make!(User);
///       ^^^^   User has an exact occurrence inside the input
/// ^^^^^^^^^^^^ Generated can use the whole invocation as a navigation destination
/// ```
///
/// References can report the exact `User` range. Before rename edits that range, it also needs
/// to know which other parts of the expansion use the same input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct SourceLocation {
    /// UTF-8 byte offsets in the original source file.
    pub span: Span,
    pub kind: SourceLocationKind,
}

impl SourceLocation {
    pub fn direct(span: Span) -> Self {
        Self {
            span,
            kind: SourceLocationKind::Direct,
        }
    }

    pub fn is_exact(self) -> bool {
        self.kind != SourceLocationKind::NavigationFallback
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum SourceLocationKind {
    /// Syntax written directly in a source file.
    Direct,
    /// Exact written range of syntax copied into a macro expansion.
    MacroArgument,
    /// The invocation, used as a navigation destination for generated syntax.
    NavigationFallback,
}
