use wincode::{SchemaRead, SchemaWrite};

use crate::{BindingId, ScopeId, items::ItemTreeId};
use rg_std::MemorySize;

/// One lexical scope.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct ScopeData {
    pub parent: Option<ScopeId>,
    pub source_items: Vec<ItemTreeId>,
    pub bindings: Vec<BindingId>,
}

impl ScopeData {
    pub fn shrink_to_fit(&mut self) {
        self.source_items.shrink_to_fit();
        self.bindings.shrink_to_fit();
    }
}
