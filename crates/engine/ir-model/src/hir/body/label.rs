use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use rg_parse::Span;
use rg_text::Name;

/// A loop label written on loop-like syntax or referenced from a jump expression.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct LabelData {
    pub name: Name,
    #[shrink(skip)]
    pub span: Span,
}

impl LabelData {
    pub fn shrink_to_fit(&mut self) {
        Shrink::shrink_to_fit(self);
    }
}
