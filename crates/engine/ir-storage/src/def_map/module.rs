use rg_ir_model::items::Documentation;
use rg_ir_model::{ImportId, LocalDefId, LocalImplId, ModuleId};
use rg_parse::{FileId, Span};
use rg_std::{MemorySize, Shrink};
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

use super::scope::ModuleScope;

/// One module in the frozen namespace graph.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ModuleData {
    pub name: Option<Name>,
    pub name_span: Option<Span>,
    pub docs: Option<Documentation>,
    pub parent: Option<ModuleId>,
    pub children: Vec<(Name, ModuleId)>,
    pub local_defs: Vec<LocalDefId>,
    pub impls: Vec<LocalImplId>,
    pub imports: Vec<ImportId>,
    pub unresolved_imports: Vec<ImportId>,
    pub scope: ModuleScope,
    pub origin: ModuleOrigin,
}

/// Where a module-like scope came from.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub enum ModuleOrigin {
    /// Root module of the crate, it is nameless (corresponds to the `crate::` scope).
    Root { file_id: FileId },
    /// Synthetic nameless module, e.g. a scope in the function body.
    /// This kind of module is used to create def maps for bodies, where we have a
    /// hierarchical structure, but can't use "real" module semantics, since the rules
    /// for items in body scopes are different from the normal module rules.
    Synthetic { file_id: FileId, span: Span },
    /// Inline module, like `mod foo { ...  }`;
    Inline {
        declaration_file: FileId,
        declaration_span: Span,
    },
    /// Out-of-line module, like `mod foo;`.
    OutOfLine {
        declaration_file: FileId,
        declaration_span: Span,
        definition_file: Option<FileId>,
    },
}

impl ModuleOrigin {
    /// Returns whether this module's source touches the requested file.
    pub fn contains_file(&self, file_id: FileId) -> bool {
        match self {
            Self::Root { file_id: root_file } => *root_file == file_id,
            Self::Synthetic {
                file_id: synthetic_file,
                ..
            } => *synthetic_file == file_id,
            Self::Inline {
                declaration_file, ..
            } => *declaration_file == file_id,
            Self::OutOfLine {
                declaration_file,
                definition_file,
                ..
            } => *declaration_file == file_id || *definition_file == Some(file_id),
        }
    }
}
