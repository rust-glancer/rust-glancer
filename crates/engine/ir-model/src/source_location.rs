use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::Span;

/// A source occurrence after resolving it to a physical file range in the selected snapshot.
/// The caller still supplies the file identity; this value only describes the range within it.
///
/// A navigation destination can exist without an editable occurrence:
///
/// ```text
/// macro_rules! make { ($name:ident) => { struct $name; struct Generated; }; }
/// make!(User);
///       ^^^^   the copied name has a real occurrence here
/// ^^^^^^^^^^^^ Generated navigates to the invocation, but its name is not written here
/// ```
///
/// Both results can be used for navigation. Only the first can be treated as an occurrence of the
/// declared name. Replacing the second range would replace the whole invocation.
///
/// `editable` proves that the range represents source syntax, not that every editing operation is
/// valid. Rename still needs its own checks, such as whether changing macro input affects several
/// expanded declarations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct SourceLocation {
    /// UTF-8 byte offsets in the source file, never offsets in temporary expansion syntax.
    pub span: Span,
    /// False when `span` is only a navigation fallback rather than this syntax's occurrence.
    pub editable: bool,
}
