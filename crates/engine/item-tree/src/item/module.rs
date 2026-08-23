use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use super::{Documentation, ItemTreeId, MacroUseAttr};

/// Syntactic module facts attached to `ItemKind::Module`.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ModuleItem {
    pub inner_docs: Option<Documentation>,
    pub macro_use: Option<MacroUseAttr>,
    /// Direct string-literal `#[path]` used while semantic traversal resolves this declaration.
    pub path_override: Option<String>,
    pub source: ModuleSource,
}

/// How a module declaration obtains its item list.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum ModuleSource {
    Inline {
        items: Vec<ItemTreeId>,
    },
    /// File selection is deliberately absent from ItemTree. The same declaration can resolve
    /// differently when its file is included or otherwise reached through another module context.
    OutOfLine,
}
