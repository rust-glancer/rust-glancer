use rg_ir_model::{LocalDefRef, SourceId};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Resolved macro invocation written inside a body.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodyMacroCallData {
    /// Whole macro call source, e.g. `format!("{}", value)`.
    pub source: SourceId,
    /// Invoked macro name occurrence used by cursor queries, e.g. `format`.
    pub name_location: SourceId,
    /// Macro definition selected by the same lookup policy used for body expansion.
    pub definition: LocalDefRef,
}
