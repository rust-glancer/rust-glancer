use rg_parse::FileId;

use super::{Documentation, ItemTreeId};

/// Syntactic module facts attached to `ItemKind::Module`.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ModuleItem {
    pub inner_docs: Option<Documentation>,
    pub source: ModuleSource,
}

/// How a module declaration obtains its item list.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ModuleSource {
    Inline { items: Vec<ItemTreeId> },
    OutOfLine { definition_file: Option<FileId> },
}
