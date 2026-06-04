use rg_ir_model::items::Documentation;
use rg_ir_model::{ImportId, LocalDefId, LocalImplId, ModuleId};
use rg_parse::{FileId, Span};
use rg_text::Name;

use super::scope::ModuleScope;

/// One module in the frozen namespace graph.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
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

impl ModuleData {
    pub(crate) fn shrink_to_fit(&mut self) {
        if let Some(name) = &mut self.name {
            name.shrink_to_fit();
        }
        if let Some(docs) = &mut self.docs {
            docs.shrink_to_fit();
        }
        self.children.shrink_to_fit();
        for (name, _) in &mut self.children {
            name.shrink_to_fit();
        }
        self.local_defs.shrink_to_fit();
        self.impls.shrink_to_fit();
        self.imports.shrink_to_fit();
        self.unresolved_imports.shrink_to_fit();
        self.scope.shrink_to_fit();
    }
}

/// Where a module-like scope came from.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
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
