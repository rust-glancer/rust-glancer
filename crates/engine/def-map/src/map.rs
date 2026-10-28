use std::{collections::HashMap, convert::Infallible, mem};

use rg_arena::Arena;
use rg_ir_model::{
    DefMapRef, FileId, ImportId, LocalDefId, LocalDefRef, LocalEnumVariantId, LocalEnumVariantRef,
    LocalImplId, LocalImplRef, ModuleId, ModuleRef, Span,
};
use rg_item_tree::{EnumItem, FieldList, ItemKind, UserFacingAttrs, VisibilityLevel};
use rg_std::{MemoryRecorder, MemorySize, Shrink};
use rg_text::{Name, identifier_text};
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    ItemSource,
    import::ImportData,
    local::{
        LocalDefData, LocalEnumVariantData, LocalEnumVariantEntry, LocalImplData,
        MacroDefinitionData,
    },
    module::{ModuleData, ModuleDocumentation, ModuleOrigin, ModuleResolutionView},
    scope::{ModuleScopeBuilder, Namespace, NamespaceSet, PerNs, ScopeBinding, Visibility},
    source::{GeneratedItemStore, GeneratedSourceData, GeneratedSourceId},
};

#[derive(Debug, Default, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
struct DefMapData {
    modules: Arena<ModuleId, ModuleData>,
    local_defs: Arena<LocalDefId, LocalDefData>,
    local_enum_variants: Arena<LocalEnumVariantId, LocalEnumVariantData>,
    macro_definitions: HashMap<LocalDefId, MacroDefinitionData>,
    /// Sparse ownership for declarations collected through an extern block.
    foreign_blocks: HashMap<LocalDefId, ItemSource>,
    local_impls: Arena<LocalImplId, LocalImplData>,
    imports: Arena<ImportId, ImportData>,
}

/// One module while declarations and imports are still being collected.
///
/// Declarations, child modules, extern crates and imported names enter the same retained scope.
/// For `struct Local; use crate::other::Remote as Alias;`, it first contains `Local`, then gains
/// `Alias` when its import resolves. Macro-generated declarations use that same scope too. Here
/// "direct" describes how a binding arrived; it does not mean there is a separate declaration-only
/// scope to replay imports from.
///
/// Import collection takes the buffer and returns it before macro lookup resumes. Winning import
/// routes live only in mutable namespace slots; freezing keeps the selected bindings and
/// unresolved-import report in `ModuleData`, without the collector's scheduling state.
#[derive(Debug)]
pub struct ModuleBuilder {
    pub name: Option<Name>,
    pub name_span: Option<Span>,
    pub docs: Option<ModuleDocumentation>,
    pub user_facing_attrs: UserFacingAttrs,
    pub visibility: Visibility,
    pub parent: Option<ModuleId>,
    pub children: Vec<(Name, ModuleId)>,
    pub local_defs: Vec<LocalDefId>,
    pub impls: Vec<LocalImplId>,
    pub imports: Vec<ImportId>,
    pub origin: ModuleOrigin,
    scope: ModuleScopeBuilder,
    unresolved_imports: Vec<ImportId>,
}

impl ModuleBuilder {
    pub fn new(origin: ModuleOrigin, visibility: Visibility, parent: Option<ModuleId>) -> Self {
        Self {
            name: None,
            name_span: None,
            docs: None,
            user_facing_attrs: UserFacingAttrs::default(),
            visibility,
            parent,
            children: Vec::new(),
            local_defs: Vec::new(),
            impls: Vec::new(),
            imports: Vec::new(),
            origin,
            scope: ModuleScopeBuilder::default(),
            unresolved_imports: Vec::new(),
        }
    }

    pub fn resolution_view(&self) -> ModuleResolutionView<'_> {
        ModuleResolutionView {
            parent: self.parent,
            visibility: self.visibility,
            origin: &self.origin,
        }
    }

    fn freeze(self) -> ModuleData {
        ModuleData {
            name: self.name,
            name_span: self.name_span,
            docs: self.docs,
            user_facing_attrs: self.user_facing_attrs,
            visibility: self.visibility,
            parent: self.parent,
            children: self.children,
            local_defs: self.local_defs,
            impls: self.impls,
            imports: self.imports,
            unresolved_imports: self.unresolved_imports,
            scope: self.scope.freeze(),
            origin: self.origin,
        }
    }
}

/// Collects declarations and import results before producing a frozen DefMap.
///
/// Collectors add modules, items, and direct bindings here. Import resolution retains those
/// bindings and adds imported answers to the same scope.
///
/// Crate builds also retain generated syntax for Semantic IR. Freezing moves that syntax out
/// separately, so the queryable DefMap does not keep the expansion arena alive.
#[derive(Debug)]
pub struct DefMapBuilder {
    own_ref: DefMapRef,
    modules: Arena<ModuleId, ModuleBuilder>,
    local_defs: Arena<LocalDefId, LocalDefData>,
    local_enum_variants: Arena<LocalEnumVariantId, LocalEnumVariantData>,
    macro_definitions: HashMap<LocalDefId, MacroDefinitionData>,
    foreign_blocks: HashMap<LocalDefId, ItemSource>,
    local_impls: Arena<LocalImplId, LocalImplData>,
    imports: Arena<ImportId, ImportData>,
    generated_items: GeneratedItemStore,
}

impl DefMapBuilder {
    pub fn new(own_ref: DefMapRef) -> Self {
        Self {
            own_ref,
            modules: Arena::new(),
            local_defs: Arena::new(),
            local_enum_variants: Arena::new(),
            macro_definitions: HashMap::new(),
            foreign_blocks: HashMap::new(),
            local_impls: Arena::new(),
            imports: Arena::new(),
            generated_items: GeneratedItemStore::default(),
        }
    }

    pub fn module(&self, module_id: ModuleId) -> Option<&ModuleBuilder> {
        self.modules.get(module_id)
    }

    pub fn module_mut(&mut self, module_id: ModuleId) -> Option<&mut ModuleBuilder> {
        self.modules.get_mut(module_id)
    }

    pub fn module_count(&self) -> usize {
        self.modules.len()
    }

    /// Allocate all construction state for one module under a stable module id.
    pub fn alloc_module(&mut self, module: ModuleBuilder) -> ModuleId {
        self.modules.alloc(module)
    }

    /// Collect a declaration into the scope used by both import and macro lookup.
    ///
    /// Expansion can add a declaration after imports have already populated the scope. Select it
    /// against the existing answer too: a declaration shadows a glob, while equal-priority
    /// conflicts keep the earlier answer.
    pub fn insert_direct_binding(
        &mut self,
        module: ModuleId,
        name: &Name,
        namespace: Namespace,
        binding: ScopeBinding,
    ) {
        let binding = self
            .module_scope(module)
            .and_then(|scope| scope.binding(name, namespace))
            .map_or(binding, |existing| {
                existing
                    .select(binding, |left, right| {
                        Ok::<_, Infallible>(self.broadest_visibility(left, right))
                    })
                    .expect("local visibility comparison is infallible")
            });
        self.modules
            .get_mut(module)
            .expect("binding module should exist")
            .scope
            .set_binding(name, namespace, binding);
    }

    /// Lend the scope buffer to import resolution; return it before macro lookup or freezing.
    ///
    /// The module keeps its metadata and an empty scope placeholder. Import lookup must use the
    /// resolver's overlay until `apply_module_imports` puts the updated buffer back.
    pub fn take_module_scope(&mut self, module: ModuleId) -> ModuleScopeBuilder {
        mem::take(
            &mut self
                .modules
                .get_mut(module)
                .expect("import module exists")
                .scope,
        )
    }

    /// Read declarations and retained import answers.
    pub fn module_scope(&self, module: ModuleId) -> Option<&ModuleScopeBuilder> {
        self.modules.get(module).map(|module| &module.scope)
    }

    pub(crate) fn module_scope_mut(&mut self, module: ModuleId) -> Option<&mut ModuleScopeBuilder> {
        self.modules.get_mut(module).map(|module| &mut module.scope)
    }

    /// Return the scope buffer and its unresolved-import report after import collection.
    /// A glob can resolve to an empty module, so the report can change even without new bindings.
    pub fn apply_module_imports(
        &mut self,
        module: ModuleId,
        scope: ModuleScopeBuilder,
        unresolved_imports: Vec<ImportId>,
    ) {
        let module = self
            .modules
            .get_mut(module)
            .expect("import module should exist");
        module.scope = scope;
        module.unresolved_imports = unresolved_imports;
    }

    pub fn alloc_local_def(&mut self, local_def: LocalDefData) -> LocalDefId {
        self.local_defs.alloc(local_def)
    }

    pub fn alloc_local_enum_variant(
        &mut self,
        local_enum_variant: LocalEnumVariantData,
    ) -> LocalEnumVariantId {
        self.local_enum_variants.alloc(local_enum_variant)
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
                user_facing_attrs: variant.user_facing_attrs,
            });
        }
    }

    pub fn insert_macro_definition(
        &mut self,
        local_def: LocalDefId,
        macro_definition: MacroDefinitionData,
    ) {
        self.macro_definitions.insert(local_def, macro_definition);
    }

    /// Records the extern block that owns one otherwise-ordinary local definition.
    ///
    /// This stays in a sparse map so normal Rust declarations do not pay a per-item memory cost.
    pub fn insert_foreign_block(&mut self, local_def: LocalDefId, block: ItemSource) {
        self.foreign_blocks.insert(local_def, block);
    }

    pub fn alloc_local_impl(&mut self, local_impl: LocalImplData) -> LocalImplId {
        self.local_impls.alloc(local_impl)
    }

    /// Records the generated items that replace one trait/impl macro invocation.
    pub(crate) fn insert_associated_macro_expansion(
        &mut self,
        call: ItemSource,
        generated_items: Vec<ItemSource>,
    ) {
        self.generated_items
            .insert_associated_macro_expansion(call, generated_items);
    }

    pub fn alloc_import(&mut self, import: ImportData) -> ImportId {
        self.imports.alloc(import)
    }

    pub(crate) fn alloc_generated_source(
        &mut self,
        generated_source: GeneratedSourceData,
    ) -> GeneratedSourceId {
        self.generated_items.alloc_source(generated_source)
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
            origin: self.own_ref,
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

    /// Valid restrictions on one module's binding form an ancestor chain. Keep the first region
    /// if incomplete syntax produced unrelated restrictions instead of widening it arbitrarily.
    fn broadest_visibility(&self, left: Visibility, right: Visibility) -> Visibility {
        match (left, right) {
            (Visibility::Public, _) | (_, Visibility::Public) => Visibility::Public,
            (Visibility::Invisible, visibility) | (visibility, Visibility::Invisible) => visibility,
            (Visibility::Module(left), Visibility::Module(right)) => {
                if left.origin == self.own_ref
                    && right.origin == self.own_ref
                    && self.module_is_descendant_of(left.module, right.module)
                {
                    Visibility::Module(right)
                } else {
                    Visibility::Module(left)
                }
            }
        }
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
        self.module(module)?.parent
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
                name => self
                    .module(current)?
                    .children
                    .iter()
                    .find_map(|(child_name, child)| {
                        (child_name == identifier_text(name)).then_some(*child)
                    })?,
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

    pub(crate) fn local_defs(&self) -> &[LocalDefData] {
        self.local_defs.as_slice()
    }

    /// Returns local definition data allocated so far during construction.
    pub fn local_def(&self, local_def: LocalDefId) -> Option<&LocalDefData> {
        self.local_defs.get(local_def)
    }

    /// Returns enum variant data allocated so far during construction.
    pub fn local_enum_variant(
        &self,
        local_enum_variant: LocalEnumVariantId,
    ) -> Option<&LocalEnumVariantData> {
        self.local_enum_variants.get(local_enum_variant)
    }

    pub fn local_enum_variant_entries_for_enum(
        &self,
        enum_def: LocalDefId,
    ) -> impl Iterator<Item = LocalEnumVariantEntry<'_>> {
        self.local_enum_variants
            .iter_with_ids()
            .filter(move |(_, variant)| variant.enum_def == enum_def)
            .map(move |(id, data)| LocalEnumVariantEntry::new(self.own_ref, id, data))
    }

    /// Returns a declarative macro payload allocated so far during construction.
    pub fn macro_definition(&self, local_def: LocalDefId) -> Option<&MacroDefinitionData> {
        self.macro_definitions.get(&local_def)
    }

    /// Returns imports allocated so far during construction.
    pub fn imports(&self) -> &[ImportData] {
        self.imports.as_slice()
    }

    pub fn imports_with_ids(&self) -> impl Iterator<Item = (ImportId, &ImportData)> {
        self.imports.iter_with_ids()
    }

    /// Returns a generated source allocated so far during macro collection.
    pub(crate) fn generated_source(
        &self,
        generated_source: GeneratedSourceId,
    ) -> Option<&GeneratedSourceData> {
        self.generated_items.source(generated_source)
    }

    pub fn build(self) -> DefMap {
        let (def_map, generated_items) = self.into_parts();
        debug_assert!(
            generated_items.is_empty(),
            "generated crate declarations must be handed to Semantic IR",
        );
        def_map
    }

    /// Freeze each module and move declaration arenas into the retained map.
    /// Generated syntax leaves separately for Semantic IR lowering.
    pub(crate) fn into_parts(self) -> (DefMap, GeneratedItemStore) {
        // Keep allocation order: parent links, imports, and bindings already contain these ids.
        let modules = Arena::from_vec(
            self.modules
                .into_vec()
                .into_iter()
                .map(ModuleBuilder::freeze)
                .collect(),
        );
        let def_map = DefMap {
            own_ref: self.own_ref,
            data: DefMapData {
                modules,
                local_defs: self.local_defs,
                local_enum_variants: self.local_enum_variants,
                macro_definitions: self.macro_definitions,
                foreign_blocks: self.foreign_blocks,
                local_impls: self.local_impls,
                imports: self.imports,
            },
        };
        (def_map, self.generated_items)
    }
}

/// Frozen namespace map for one analyzed scope.
///
/// There might be several defmaps per crate:
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

    /// Returns the retained extern-block owner for a foreign declaration.
    pub fn foreign_block(&self, local_def: LocalDefId) -> Option<ItemSource> {
        self.data.foreign_blocks.get(&local_def).copied()
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

    pub fn import(&self, import: ImportId) -> Option<&ImportData> {
        self.data.imports.get(import)
    }

    pub fn imports_with_ids(&self) -> impl Iterator<Item = (ImportId, &ImportData)> {
        self.data.imports.iter_with_ids()
    }
}

impl MemorySize for DefMap {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("data", |recorder| {
            MemorySize::record_memory_children(&self.data, recorder);
        });
    }
}
