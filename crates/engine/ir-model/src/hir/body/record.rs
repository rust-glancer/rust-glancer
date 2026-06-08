use rg_std::MemorySize;
use wincode::{SchemaRead, SchemaWrite};

/// Whether a record field was written with `key: value` syntax or colonless shorthand.
///
/// This is shared by record expressions and record patterns so later source queries can preserve
/// the user's spelling without carrying inverted `explicit`/`shorthand` booleans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
#[memsize(leaf)]
pub enum RecordFieldSyntax {
    /// `User { name: value }` or `User { name: ref binding }`.
    Explicit,
    /// `User { name }` or `User { ref name }`.
    Shorthand,
}

impl RecordFieldSyntax {
    pub fn is_explicit(self) -> bool {
        matches!(self, Self::Explicit)
    }

    pub fn is_shorthand(self) -> bool {
        matches!(self, Self::Shorthand)
    }
}
