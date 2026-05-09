use std::collections::HashMap;

use rg_arena::Arena;
use rg_item_tree::{Documentation, ItemTag, ItemTreeRef, VisibilityLevel};
use rg_parse::{FileId, Span};
use rg_text::Name;

use super::{ImportData, ImportId, LocalDefId, LocalImplId, ModuleId, ModuleRef, ModuleScope};
use crate::scope::Namespace;

/// Frozen namespace map for one analyzed target.
#[derive(
    Debug, Clone, PartialEq, Eq, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct DefMap {
    pub(crate) root_module: Option<ModuleId>,
    // Currently means “implicit roots visible to this target,” including sibling lib roots
    pub(crate) extern_prelude: HashMap<Name, ModuleRef>,
    // Standard prelude module selected for this target, if sysroot sources are available.
    pub(crate) prelude: Option<ModuleRef>,
    pub(crate) modules: Arena<ModuleId, ModuleData>,
    pub(crate) local_defs: Arena<LocalDefId, LocalDefData>,
    pub(crate) local_impls: Arena<LocalImplId, LocalImplData>,
    pub(crate) imports: Arena<ImportId, ImportData>,
}

impl DefMap {
    /// Returns the root module of this target, if the map has been populated.
    pub(crate) fn root_module(&self) -> Option<ModuleId> {
        self.root_module
    }

    /// Returns the external root names visible from this target.
    pub(crate) fn extern_prelude(&self) -> &HashMap<Name, ModuleRef> {
        &self.extern_prelude
    }

    /// Returns the standard prelude module visible from this target, if it was discovered.
    pub(crate) fn prelude(&self) -> Option<ModuleRef> {
        self.prelude
    }

    /// Returns all modules in stable module-id order.
    pub fn modules(&self) -> &[ModuleData] {
        self.modules.as_slice()
    }

    /// Returns module data by id.
    pub fn module(&self, module_id: ModuleId) -> Option<&ModuleData> {
        self.modules.get(module_id)
    }

    /// Returns local definition data by id.
    pub(crate) fn local_def(&self, local_def: LocalDefId) -> Option<&LocalDefData> {
        self.local_defs.get(local_def)
    }

    /// Returns all local definitions in stable local-def-id order.
    pub(crate) fn local_defs(&self) -> &[LocalDefData] {
        self.local_defs.as_slice()
    }

    /// Returns impl block data by id.
    pub(crate) fn local_impl(&self, local_impl: LocalImplId) -> Option<&LocalImplData> {
        self.local_impls.get(local_impl)
    }

    /// Returns all impl blocks in stable local-impl-id order.
    pub(crate) fn local_impls(&self) -> &[LocalImplData] {
        self.local_impls.as_slice()
    }

    /// Returns all imports in stable import-id order.
    pub fn imports(&self) -> &[ImportData] {
        self.imports.as_slice()
    }

    pub(super) fn set_root_module(&mut self, root_module: ModuleId) {
        self.root_module = Some(root_module);
    }

    pub(super) fn set_extern_prelude(&mut self, extern_prelude: HashMap<Name, ModuleRef>) {
        self.extern_prelude = extern_prelude;
    }

    pub(super) fn set_prelude(&mut self, prelude: Option<ModuleRef>) {
        self.prelude = prelude;
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.extern_prelude.shrink_to_fit();
        self.modules.shrink_to_fit();
        for module in self.modules.iter_mut() {
            module.shrink_to_fit();
        }
        self.local_defs.shrink_to_fit();
        for local_def in self.local_defs.iter_mut() {
            local_def.shrink_to_fit();
        }
        self.local_impls.shrink_to_fit();
        self.imports.shrink_to_fit();
        for import in self.imports.iter_mut() {
            import.shrink_to_fit();
        }
    }
}

/// One module in the frozen namespace graph.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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
    fn shrink_to_fit(&mut self) {
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

/// Where a module came from in source code.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ModuleOrigin {
    Root {
        file_id: FileId,
    },
    Inline {
        declaration_file: FileId,
        declaration_span: Span,
    },
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

/// One module-scope definition collected from source.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct LocalDefData {
    pub module: ModuleId,
    pub name: Name,
    pub kind: LocalDefKind,
    pub visibility: VisibilityLevel,
    pub source: ItemTreeRef,
    pub file_id: FileId,
    pub name_span: Option<Span>,
    pub span: Span,
}

impl LocalDefData {
    fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
    }
}

/// One module-owned impl block collected from source.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct LocalImplData {
    pub module: ModuleId,
    pub source: ItemTreeRef,
    pub file_id: FileId,
    pub span: Span,
}

/// Module-scope definition kind that participates in def-map namespaces.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub enum LocalDefKind {
    #[display("const")]
    Const,
    #[display("enum")]
    Enum,
    #[display("fn")]
    Function,
    #[display("macro_definition")]
    MacroDefinition,
    #[display("static")]
    Static,
    #[display("struct")]
    Struct,
    #[display("trait")]
    Trait,
    #[display("type_alias")]
    TypeAlias,
    #[display("union")]
    Union,
}

impl LocalDefKind {
    pub(super) fn from_item_tag(tag: ItemTag) -> Option<Self> {
        match tag {
            ItemTag::Const => Some(Self::Const),
            ItemTag::Enum => Some(Self::Enum),
            ItemTag::Function => Some(Self::Function),
            ItemTag::MacroDefinition => Some(Self::MacroDefinition),
            ItemTag::Static => Some(Self::Static),
            ItemTag::Struct => Some(Self::Struct),
            ItemTag::Trait => Some(Self::Trait),
            ItemTag::TypeAlias => Some(Self::TypeAlias),
            ItemTag::Union => Some(Self::Union),
            ItemTag::AsmExpr
            | ItemTag::ExternBlock
            | ItemTag::ExternCrate
            | ItemTag::Impl
            | ItemTag::Module
            | ItemTag::Use => None,
        }
    }

    pub(super) fn namespace(self) -> Namespace {
        match self {
            Self::Const | Self::Function | Self::Static => Namespace::Values,
            Self::Enum | Self::Struct | Self::Trait | Self::TypeAlias | Self::Union => {
                Namespace::Types
            }
            Self::MacroDefinition => Namespace::Macros,
        }
    }
}
