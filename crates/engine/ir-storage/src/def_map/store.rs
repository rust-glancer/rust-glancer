use rg_std::{MemorySize, Shrink};
use rg_text::identifier_text;
use std::collections::HashMap;
use wincode::{SchemaRead, SchemaWrite};

use rg_arena::Arena;
use rg_ir_model::{
    BodyRef, DefMapRef, ImportId, LocalDefId, LocalDefRef, LocalEnumVariantId, LocalEnumVariantRef,
    LocalImplId, LocalImplRef, ModuleId, ModuleRef, TargetRef,
    hir::source::{GeneratedSourceData, GeneratedSourceId},
    items::{EnumItem, FieldList, ItemKind, VisibilityLevel},
};
use rg_parse::FileId;

use super::{
    import::ImportData,
    local::{
        LocalDefData, LocalEnumVariantData, LocalEnumVariantEntry, LocalImplData,
        MacroDefinitionData,
    },
    module::ModuleData,
    scope::{Namespace, NamespaceSet, PerNs, Visibility},
};

#[derive(Debug, Default, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
struct DefMapData {
    modules: Arena<ModuleId, ModuleData>,
    local_defs: Arena<LocalDefId, LocalDefData>,
    local_enum_variants: Arena<LocalEnumVariantId, LocalEnumVariantData>,
    macro_definitions: HashMap<LocalDefId, MacroDefinitionData>,
    local_impls: Arena<LocalImplId, LocalImplData>,
    imports: Arena<ImportId, ImportData>,
    generated_sources: Arena<GeneratedSourceId, GeneratedSourceData>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefMapBuilder {
    def_map: DefMap,
}

impl DefMapBuilder {
    pub fn new(target: TargetRef) -> Self {
        Self {
            def_map: DefMap::target(target),
        }
    }

    pub fn new_body(body_ref: BodyRef) -> Self {
        Self {
            def_map: DefMap::body(body_ref),
        }
    }

    pub fn partial(&self) -> PartialDefMap<'_> {
        PartialDefMap {
            def_map: &self.def_map,
        }
    }

    pub fn module_mut(&mut self, module_id: ModuleId) -> Option<&mut ModuleData> {
        self.def_map.data.modules.get_mut(module_id)
    }

    pub fn alloc_module(&mut self, module: ModuleData) -> ModuleId {
        self.def_map.data.modules.alloc(module)
    }

    pub fn alloc_local_def(&mut self, local_def: LocalDefData) -> LocalDefId {
        self.def_map.data.local_defs.alloc(local_def)
    }

    pub fn alloc_local_enum_variant(
        &mut self,
        local_enum_variant: LocalEnumVariantData,
    ) -> LocalEnumVariantId {
        self.def_map
            .data
            .local_enum_variants
            .alloc(local_enum_variant)
    }

    /// Retain the variants needed by qualified paths and imports before semantic item lowering.
    pub fn alloc_local_enum_variants(
        &mut self,
        module: ModuleId,
        enum_def: LocalDefId,
        enum_item: &EnumItem,
        visibility: Visibility,
        file_id: FileId,
    ) {
        for (index, variant) in enum_item.variants.iter().enumerate() {
            self.alloc_local_enum_variant(LocalEnumVariantData {
                module,
                enum_def,
                name: variant.name.clone(),
                index,
                namespaces: NamespaceSet::for_field_list(&variant.fields),
                visibility,
                file_id,
                name_span: variant.name_span,
                span: variant.span,
            });
        }
    }

    pub fn insert_macro_definition(
        &mut self,
        local_def: LocalDefId,
        macro_definition: MacroDefinitionData,
    ) {
        self.def_map
            .data
            .macro_definitions
            .insert(local_def, macro_definition);
    }

    pub fn alloc_local_impl(&mut self, local_impl: LocalImplData) -> LocalImplId {
        self.def_map.data.local_impls.alloc(local_impl)
    }

    pub fn alloc_import(&mut self, import: ImportData) -> ImportId {
        self.def_map.data.imports.alloc(import)
    }

    pub fn alloc_generated_source(
        &mut self,
        generated_source: GeneratedSourceData,
    ) -> GeneratedSourceId {
        self.def_map.data.generated_sources.alloc(generated_source)
    }

    /// Resolves source-level visibility into the module identity used by scope lookup.
    ///
    /// Rust only permits `pub(in path)` to name an ancestor of the declaration. Collection has
    /// already built that ancestor chain, so visibility can become semantic before any import or
    /// query observes the binding. An invalid or unresolved restricted path becomes
    /// `Visibility::Invisible` rather than leaking through later lookup.
    pub fn resolve_visibility(&self, owner: ModuleId, visibility: &VisibilityLevel) -> Visibility {
        if matches!(visibility, VisibilityLevel::Public) {
            return Visibility::Public;
        }

        let visible_from = match visibility {
            VisibilityLevel::Private | VisibilityLevel::Self_ => Some(owner),
            VisibilityLevel::Crate => self.root_module(owner),
            VisibilityLevel::Super => self.parent_module(owner),
            VisibilityLevel::Restricted(path) => self.restricted_visibility_owner(owner, path),
            VisibilityLevel::Public => unreachable!("public visibility returned above"),
            VisibilityLevel::Unknown(_) => None,
        };

        let Some(visible_from) = visible_from else {
            return Visibility::Invisible;
        };
        if !self.module_is_descendant_of(owner, visible_from) {
            return Visibility::Invisible;
        }

        Visibility::Module(ModuleRef {
            origin: self.def_map.own_ref,
            module: visible_from,
        })
    }

    /// Resolves the visibility carried by each direct namespace binding for a declaration.
    ///
    /// Tuple constructors are visible only where every positional field is visible. Their type
    /// identity keeps the struct declaration's visibility, while unit constructors need no extra
    /// restriction and record structs do not occupy the value namespace at all.
    ///
    /// For `pub struct Id(u8)`, the type slot is public but the value slot is limited to the module
    /// that can see the private field.
    pub fn resolve_local_def_visibilities(
        &self,
        owner: ModuleId,
        item: &ItemKind,
        visibility: &VisibilityLevel,
    ) -> PerNs<Visibility> {
        let declaration = self.resolve_visibility(owner, visibility);
        let mut visibilities = PerNs::new(declaration, declaration, declaration);

        if let ItemKind::Struct(struct_item) = item
            && let FieldList::Tuple(fields) = &struct_item.fields
        {
            let constructor = fields.iter().fold(declaration, |constructor, field| {
                self.intersect_visibility(
                    constructor,
                    self.resolve_visibility(owner, &field.visibility),
                )
            });
            *visibilities.get_mut(Namespace::Values) = constructor;
        }

        visibilities
    }

    fn intersect_visibility(&self, left: Visibility, right: Visibility) -> Visibility {
        match (left, right) {
            (Visibility::Invisible, _) | (_, Visibility::Invisible) => Visibility::Invisible,
            (Visibility::Public, visibility) | (visibility, Visibility::Public) => visibility,
            (Visibility::Module(left), Visibility::Module(right))
                if left.origin == right.origin =>
            {
                if self.module_is_descendant_of(left.module, right.module) {
                    Visibility::Module(left)
                } else if self.module_is_descendant_of(right.module, left.module) {
                    Visibility::Module(right)
                } else {
                    Visibility::Invisible
                }
            }
            (Visibility::Module(_), Visibility::Module(_)) => Visibility::Invisible,
        }
    }

    fn parent_module(&self, module: ModuleId) -> Option<ModuleId> {
        self.def_map.module(module)?.parent
    }

    fn root_module(&self, module: ModuleId) -> Option<ModuleId> {
        let mut current = module;
        while let Some(parent) = self.parent_module(current) {
            current = parent;
        }
        Some(current)
    }

    /// Resolves `crate`, `self`, repeated `super`, and named segments in a restricted visibility.
    /// Raw names compare by semantic spelling, so `r#type` finds a module stored as `type`.
    fn restricted_visibility_owner(&self, owner: ModuleId, path: &str) -> Option<ModuleId> {
        let mut segments = path.split("::").map(str::trim);
        let first = segments.next()?;
        let mut current = match first {
            "crate" => self.root_module(owner)?,
            "self" => owner,
            "super" => self.parent_module(owner)?,
            _ => return None,
        };

        for segment in segments {
            current = match segment {
                "self" => current,
                "super" => self.parent_module(current)?,
                "crate" | "" => return None,
                name => self.def_map.module(current)?.children.iter().find_map(
                    |(child_name, child)| (child_name == identifier_text(name)).then_some(*child),
                )?,
            };
        }

        Some(current)
    }

    fn module_is_descendant_of(&self, module: ModuleId, ancestor: ModuleId) -> bool {
        let mut current = Some(module);
        while let Some(module) = current {
            if module == ancestor {
                return true;
            }
            current = self.parent_module(module);
        }
        false
    }

    pub fn build(self) -> DefMap {
        self.def_map
    }
}

/// Read-only view over a DefMap that is still being collected/finalized.
///
/// This facade is intentionally narrower than `DefMap`: it permits the build pipeline to inspect
/// data allocated so far without making the object look like a frozen namespace map.
#[derive(Debug, Clone, Copy)]
pub struct PartialDefMap<'a> {
    def_map: &'a DefMap,
}

impl<'a> PartialDefMap<'a> {
    /// Returns module data allocated so far during collection/finalization.
    pub fn module(&self, module_id: ModuleId) -> Option<&'a ModuleData> {
        self.def_map.module(module_id)
    }

    pub fn module_count(&self) -> usize {
        self.def_map.module_count()
    }

    /// Returns local definition data allocated so far during collection/finalization.
    pub fn local_def(&self, local_def: LocalDefId) -> Option<&'a LocalDefData> {
        self.def_map.local_def(local_def)
    }

    /// Returns enum variant data allocated so far during collection/finalization.
    pub fn local_enum_variant(
        &self,
        local_enum_variant: LocalEnumVariantId,
    ) -> Option<&'a LocalEnumVariantData> {
        self.def_map.local_enum_variant(local_enum_variant)
    }

    pub fn local_enum_variant_entries_for_enum(
        &self,
        enum_def: LocalDefId,
    ) -> impl Iterator<Item = LocalEnumVariantEntry<'a>> + 'a {
        self.def_map.local_enum_variant_entries_for_enum(enum_def)
    }

    /// Returns a declarative macro payload allocated so far during collection/finalization.
    pub fn macro_definition(&self, local_def: LocalDefId) -> Option<&'a MacroDefinitionData> {
        self.def_map.macro_definition(local_def)
    }

    /// Returns imports allocated so far during collection/finalization.
    pub fn imports(&self) -> &'a [ImportData] {
        self.def_map.imports()
    }

    pub fn imports_with_ids(&self) -> impl Iterator<Item = (ImportId, &'a ImportData)> + 'a {
        self.def_map.imports_with_ids()
    }

    /// Returns a retained generated source allocated so far during macro collection.
    pub fn generated_source(
        &self,
        generated_source: GeneratedSourceId,
    ) -> Option<&'a GeneratedSourceData> {
        self.def_map.generated_source(generated_source)
    }
}

/// Frozen namespace map for one analyzed scope.
///
/// There might be several defmaps per target:
/// the root defmap represents the semantic layer, but also
/// each body function has its own defmap that tracks the body-local items.
/// While functions are not really modules, they work similarly, and we model
/// them as if each scope is a module.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, Shrink)]
pub struct DefMap {
    /// Ref to this defmap, which can be used to emit correct refs.
    own_ref: DefMapRef,
    /// Actual defmap layout for the corresponding scope.
    data: DefMapData,
}

impl DefMap {
    fn target(target: TargetRef) -> Self {
        Self {
            own_ref: DefMapRef::Target(target),
            data: DefMapData::default(),
        }
    }

    fn body(body_ref: BodyRef) -> Self {
        Self {
            own_ref: DefMapRef::Body(body_ref),
            data: DefMapData::default(),
        }
    }

    pub fn own_ref(&self) -> DefMapRef {
        self.own_ref
    }

    /// Returns all modules in stable module-id order.
    pub fn modules(&self) -> &[ModuleData] {
        self.data.modules.as_slice()
    }

    /// Returns refs for all the modules in stable module-id order.
    pub fn module_refs(&self) -> impl Iterator<Item = ModuleRef> {
        (0..self.data.modules.len()).map(|id| ModuleRef {
            origin: self.own_ref,
            module: ModuleId(id),
        })
    }

    /// Returns module data by id.
    pub fn module(&self, module_id: ModuleId) -> Option<&ModuleData> {
        self.data.modules.get(module_id)
    }

    pub fn module_count(&self) -> usize {
        self.data.modules.len()
    }

    /// Returns local definition data by id.
    pub fn local_def(&self, local_def: LocalDefId) -> Option<&LocalDefData> {
        self.data.local_defs.get(local_def)
    }

    /// Returns all local definitions in stable local-def-id order.
    pub fn local_defs(&self) -> &[LocalDefData] {
        self.data.local_defs.as_slice()
    }

    pub fn local_def_refs(&self) -> impl Iterator<Item = LocalDefRef> {
        (0..self.data.local_defs.len()).map(|id| LocalDefRef {
            origin: self.own_ref,
            local_def: LocalDefId(id),
        })
    }

    /// Returns enum variant data by id.
    pub fn local_enum_variant(
        &self,
        local_enum_variant: LocalEnumVariantId,
    ) -> Option<&LocalEnumVariantData> {
        self.data.local_enum_variants.get(local_enum_variant)
    }

    /// Returns all enum variants in stable variant-id order.
    pub fn local_enum_variants(&self) -> &[LocalEnumVariantData] {
        self.data.local_enum_variants.as_slice()
    }

    pub fn local_enum_variant_refs(&self) -> impl Iterator<Item = LocalEnumVariantRef> {
        self.data
            .local_enum_variants
            .iter_with_ids()
            .map(|(id, _)| LocalEnumVariantRef {
                origin: self.own_ref,
                local_enum_variant: id,
            })
    }

    pub fn local_enum_variant_entries_for_enum(
        &self,
        enum_def: LocalDefId,
    ) -> impl Iterator<Item = LocalEnumVariantEntry<'_>> {
        let origin = self.own_ref;
        self.data
            .local_enum_variants
            .iter_with_ids()
            .filter(move |(_, variant)| variant.enum_def == enum_def)
            .map(move |(id, data)| LocalEnumVariantEntry::new(origin, id, data))
    }

    /// Returns a declarative macro payload by its local definition id.
    pub fn macro_definition(&self, local_def: LocalDefId) -> Option<&MacroDefinitionData> {
        self.data.macro_definitions.get(&local_def)
    }

    /// Returns impl block data by id.
    pub fn local_impl(&self, local_impl: LocalImplId) -> Option<&LocalImplData> {
        self.data.local_impls.get(local_impl)
    }

    /// Returns all impl blocks in stable local-impl-id order.
    pub fn local_impls(&self) -> &[LocalImplData] {
        self.data.local_impls.as_slice()
    }

    pub fn local_impl_refs(&self) -> impl Iterator<Item = LocalImplRef> {
        (0..self.data.local_impls.len()).map(|id| LocalImplRef {
            origin: self.own_ref,
            local_impl: LocalImplId(id),
        })
    }

    /// Returns all imports in stable import-id order.
    pub fn imports(&self) -> &[ImportData] {
        self.data.imports.as_slice()
    }

    /// Returns one retained generated source by id.
    pub fn generated_source(
        &self,
        generated_source: GeneratedSourceId,
    ) -> Option<&GeneratedSourceData> {
        self.data.generated_sources.get(generated_source)
    }

    /// Returns all retained generated sources in stable generated-source-id order.
    pub fn generated_sources(&self) -> &[GeneratedSourceData] {
        self.data.generated_sources.as_slice()
    }

    pub fn imports_with_ids(&self) -> impl Iterator<Item = (ImportId, &ImportData)> {
        self.data.imports.iter_with_ids()
    }
}

impl MemorySize for DefMap {
    fn record_memory_children(&self, recorder: &mut rg_std::MemoryRecorder) {
        recorder.scope("data", |recorder| {
            MemorySize::record_memory_children(&self.data, recorder);
        });
    }
}
