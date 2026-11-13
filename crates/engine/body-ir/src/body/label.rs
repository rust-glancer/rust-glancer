use rg_ir_model::SourceId;
use rg_std::{MemorySize, Shrink};
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

/// A loop label written on loop-like syntax or referenced from a jump expression.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct LabelData {
    pub name: Name,
    #[shrink(skip)]
    pub location: SourceId,
}
