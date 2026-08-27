use rg_ir_model::{ImportId, LocalDefId, LocalImplId, ModuleId};
use rg_item_tree::{Documentation, UserFacingAttrs};
use rg_parse::{FileId, Span};
use rg_std::{MemorySize, Shrink};
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

use crate::scope::{ModuleScope, Visibility};

/// One module in the frozen namespace graph.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ModuleData {
    pub name: Option<Name>,
    pub name_span: Option<Span>,
    pub docs: Option<Documentation>,
    pub user_facing_attrs: UserFacingAttrs,
    /// Visibility of the declaration that introduced this module identity.
    ///
    /// Keyword imports such as `use super as parent` do not pass through the parent's textual name,
    /// so the module itself must retain the same visibility ceiling as its direct scope binding.
    pub visibility: Visibility,
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
        /// How the definition file was selected.
        ///
        /// A `#[path]` file resolves its own children beside that file, even when its filename
        /// looks like an ordinary flat module. Keeping this provenance avoids retaining complete
        /// filesystem contexts in every frozen module.
        file_selection: ModuleFileSelection,
    },
}

/// Source rule that selected an out-of-line module's definition file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum ModuleFileSelection {
    /// Ordinary `name.rs` or `name/mod.rs` lookup.
    Conventional,
    /// A direct `#[path = "..."]` attribute.
    PathAttribute,
}

impl ModuleFileSelection {
    pub fn from_path_override(path_override: Option<&str>) -> Self {
        if path_override.is_some() {
            Self::PathAttribute
        } else {
            Self::Conventional
        }
    }
}

impl ModuleOrigin {
    /// Iterates every source file that participates in this module origin.
    ///
    /// An out-of-line module contributes both the file containing `mod child;` and the resolved
    /// definition file. Other origin forms contribute their single source file.
    pub(crate) fn files(&self) -> impl Iterator<Item = FileId> {
        let files = match self {
            Self::Root { file_id } | Self::Synthetic { file_id, .. } => [Some(*file_id), None],
            Self::Inline {
                declaration_file, ..
            } => [Some(*declaration_file), None],
            Self::OutOfLine {
                declaration_file,
                definition_file,
                ..
            } => [Some(*declaration_file), *definition_file],
        };
        files.into_iter().flatten()
    }

    /// Returns whether this module's source touches the requested file.
    pub fn contains_file(&self, file_id: FileId) -> bool {
        self.files().any(|candidate| candidate == file_id)
    }
}
