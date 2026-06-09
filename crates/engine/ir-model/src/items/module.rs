use rg_parse::FileId;
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use super::{Documentation, ItemTreeId, MacroUseAttr};

/// Syntactic module facts attached to `ItemKind::Module`.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ModuleItem {
    pub inner_docs: Option<Documentation>,
    pub macro_use: Option<MacroUseAttr>,
    pub source: ModuleSource,
}

impl ModuleItem {
    pub fn shrink_to_fit(&mut self) {
        Shrink::shrink_to_fit(self);
    }
}

/// How a module declaration obtains its item list.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum ModuleSource {
    Inline { items: Vec<ItemTreeId> },
    OutOfLine { definition_file: Option<FileId> },
}

impl ModuleSource {
    pub fn shrink_to_fit(&mut self) {
        Shrink::shrink_to_fit(self);
    }
}
