use wincode::{SchemaRead, SchemaWrite};

use crate::{BindingId, ScopeId, items::ItemTreeId};
use rg_std::{MemorySize, Shrink};

/// One lexical scope.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ScopeData {
    pub parent: Option<ScopeId>,
    pub source_items: Vec<ItemTreeId>,
    pub bindings: Vec<BindingId>,
}
