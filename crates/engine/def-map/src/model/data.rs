use std::collections::HashMap;

use rg_arena::Arena;
use rg_item_tree::{Documentation, ItemTag, MacroDefinitionItem, VisibilityLevel};
use rg_parse::{FileId, Span};
use rg_text::Name;
use rg_tt::TopSubtree;
use rg_workspace::RustEdition;

use super::scope::Namespace;
use super::{
    GeneratedSourceData, GeneratedSourceId, ImportData, ImportId, ItemSource, LocalDefId,
    LocalImplId, ModuleId, ModuleRef, ModuleScope, TargetRef,
};

/// Frozen namespace map for one analyzed target.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Default,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct DefMap {
    root_module: Option<ModuleId>,
    // Currently means “implicit roots visible to this target,” including sibling lib roots
    extern_prelude: HashMap<Name, ModuleRef>,
    // Standard prelude module selected for this target, if sysroot sources are available.
    prelude: Option<ModuleRef>,
    modules: Arena<ModuleId, ModuleData>,
    local_defs: Arena<LocalDefId, LocalDefData>,
    macro_definitions: HashMap<LocalDefId, MacroDefinitionData>,
    local_impls: Arena<LocalImplId, LocalImplData>,
    imports: Arena<ImportId, ImportData>,
    generated_sources: Arena<GeneratedSourceId, GeneratedSourceData>,
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

    pub(crate) fn module_mut(&mut self, module_id: ModuleId) -> Option<&mut ModuleData> {
        self.modules.get_mut(module_id)
    }

    pub(crate) fn module_count(&self) -> usize {
        self.modules.len()
    }

    /// Returns local definition data by id.
    pub(crate) fn local_def(&self, local_def: LocalDefId) -> Option<&LocalDefData> {
        self.local_defs.get(local_def)
    }

    /// Returns all local definitions in stable local-def-id order.
    pub(crate) fn local_defs(&self) -> &[LocalDefData] {
        self.local_defs.as_slice()
    }

    /// Returns a declarative macro payload by its local definition id.
    pub(crate) fn macro_definition(&self, local_def: LocalDefId) -> Option<&MacroDefinitionData> {
        self.macro_definitions.get(&local_def)
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

    /// Returns one retained generated source by id.
    pub fn generated_source(
        &self,
        generated_source: GeneratedSourceId,
    ) -> Option<&GeneratedSourceData> {
        self.generated_sources.get(generated_source)
    }

    /// Returns all retained generated sources in stable generated-source-id order.
    pub fn generated_sources(&self) -> &[GeneratedSourceData] {
        self.generated_sources.as_slice()
    }

    pub(crate) fn imports_with_ids(&self) -> impl Iterator<Item = (ImportId, &ImportData)> {
        self.imports.iter_with_ids()
    }

    pub(crate) fn alloc_module(&mut self, module: ModuleData) -> ModuleId {
        self.modules.alloc(module)
    }

    pub(crate) fn alloc_local_def(&mut self, local_def: LocalDefData) -> LocalDefId {
        self.local_defs.alloc(local_def)
    }

    pub(crate) fn insert_macro_definition(
        &mut self,
        local_def: LocalDefId,
        macro_definition: MacroDefinitionData,
    ) {
        self.macro_definitions.insert(local_def, macro_definition);
    }

    pub(crate) fn alloc_local_impl(&mut self, local_impl: LocalImplData) -> LocalImplId {
        self.local_impls.alloc(local_impl)
    }

    pub(crate) fn alloc_import(&mut self, import: ImportData) -> ImportId {
        self.imports.alloc(import)
    }

    pub(crate) fn alloc_generated_source(
        &mut self,
        generated_source: GeneratedSourceData,
    ) -> GeneratedSourceId {
        self.generated_sources.alloc(generated_source)
    }

    pub(crate) fn set_root_module(&mut self, root_module: ModuleId) {
        self.root_module = Some(root_module);
    }

    pub(crate) fn set_extern_prelude(&mut self, extern_prelude: HashMap<Name, ModuleRef>) {
        self.extern_prelude = extern_prelude;
    }

    pub(crate) fn set_prelude(&mut self, prelude: Option<ModuleRef>) {
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
        self.macro_definitions.shrink_to_fit();
        for macro_definition in self.macro_definitions.values_mut() {
            macro_definition.shrink_to_fit();
        }
        self.local_impls.shrink_to_fit();
        self.imports.shrink_to_fit();
        for import in self.imports.iter_mut() {
            import.shrink_to_fit();
        }
        self.generated_sources.shrink_to_fit();
        for generated_source in self.generated_sources.iter_mut() {
            generated_source.shrink_to_fit();
        }
    }
}

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
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
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
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct LocalDefData {
    pub module: ModuleId,
    pub name: Name,
    pub kind: LocalDefKind,
    pub visibility: VisibilityLevel,
    pub source: ItemSource,
    pub file_id: FileId,
    pub name_span: Option<Span>,
    pub span: Span,
}

impl LocalDefData {
    fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
    }
}

/// Declarative macro definition payload retained for expansion after def-map freezing.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct MacroDefinitionData {
    pub edition: RustEdition,
    /// Target that `$crate` inside this macro body should resolve to when expanded.
    pub dollar_crate_target: TargetRef,
    pub payload: MacroDefinitionPayload,
}

impl MacroDefinitionData {
    pub(crate) fn from_item(
        item: &MacroDefinitionItem,
        edition: RustEdition,
        dollar_crate_target: TargetRef,
    ) -> Self {
        Self {
            edition,
            dollar_crate_target,
            payload: MacroDefinitionPayload::from_item(item),
        }
    }

    fn shrink_to_fit(&mut self) {}
}

/// Token-tree payload needed to compile a collected declarative macro.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub enum MacroDefinitionPayload {
    MacroRules {
        #[memsize(scope = "body")]
        body: Option<TopSubtree>,
    },
    MacroDef {
        args: Option<TopSubtree>,
        body: Option<TopSubtree>,
    },
}

impl MacroDefinitionPayload {
    fn from_item(item: &MacroDefinitionItem) -> Self {
        match item {
            MacroDefinitionItem::MacroRules { body, .. } => Self::MacroRules { body: body.clone() },
            MacroDefinitionItem::MacroDef { args, body } => Self::MacroDef {
                args: args.clone(),
                body: body.clone(),
            },
        }
    }
}

/// One module-owned impl block collected from source.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct LocalImplData {
    pub module: ModuleId,
    pub source: ItemSource,
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
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
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
    pub(crate) fn from_item_tag(tag: ItemTag) -> Option<Self> {
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
            | ItemTag::MacroCall
            | ItemTag::Module
            | ItemTag::Use => None,
        }
    }

    pub(crate) fn namespace(self) -> Namespace {
        match self {
            Self::Const | Self::Function | Self::Static => Namespace::Values,
            Self::Enum | Self::Struct | Self::Trait | Self::TypeAlias | Self::Union => {
                Namespace::Types
            }
            Self::MacroDefinition => Namespace::Macros,
        }
    }
}
