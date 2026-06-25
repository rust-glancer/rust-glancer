use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::LocalDefRef;
use rg_parse::Span;

use super::BodySource;

/// Resolved macro invocation written inside a body.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodyMacroCallData {
    /// Whole macro call source, e.g. `format!("{}", value)`.
    pub source: BodySource,
    /// Invoked macro name span used by cursor queries, e.g. `format`.
    pub name_span: Span,
    /// Macro definition selected by the same lookup policy used for body expansion.
    pub definition: LocalDefRef,
}
