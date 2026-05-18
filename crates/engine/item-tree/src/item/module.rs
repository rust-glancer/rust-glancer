use rg_parse::FileId;

use super::{Documentation, ItemTreeId};

/// Syntactic module facts attached to `ItemKind::Module`.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct ModuleItem {
    pub inner_docs: Option<Documentation>,
    pub source: ModuleSource,
}

impl ModuleItem {
    pub(crate) fn shrink_to_fit(&mut self) {
        if let Some(docs) = &mut self.inner_docs {
            docs.shrink_to_fit();
        }
        self.source.shrink_to_fit();
    }
}

/// How a module declaration obtains its item list.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum ModuleSource {
    Inline { items: Vec<ItemTreeId> },
    OutOfLine { definition_file: Option<FileId> },
}

impl ModuleSource {
    fn shrink_to_fit(&mut self) {
        match self {
            Self::Inline { items } => items.shrink_to_fit(),
            Self::OutOfLine { .. } => {}
        }
    }
}
