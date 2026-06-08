use rg_std::MemorySize;
use wincode::{SchemaRead, SchemaWrite};

use rg_parse::Span;
use rg_text::Name;

/// A loop label written on loop-like syntax or referenced from a jump expression.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct LabelData {
    pub name: Name,
    pub span: Span,
}

impl LabelData {
    pub fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
    }
}
