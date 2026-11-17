//! Publish one configured target's independently readable semantic payloads.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::Arc,
};

use rg_def_map::{
    AssociatedItemInput, AssociatedItemOwner, CrateData, CrateDataSource, CrateDefMapReader,
    CrateDependencies, DefMapQuery, GeneratedItemStore, GeneratedSourceStore,
    MacroExpansionLimitReport, MacroScopeData,
};
use rg_ir_model::{
    CrateRef, DefMapRef, FileId, ItemStoreId, ItemStoreRef, LocalDefRef, SemanticItemRef,
};
use rg_item_tree::{ItemKind, ItemTrees};
use rg_source::{QueryContext, ReadError};
use rg_std::{MemorySize, Shrink as _, UniqueVec};
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

use super::{
    impl_headers::impl_header_resolutions,
    members::{AssociatedItemInputs, associated_declarations},
    symbol_names::{MemberNameData, macro_symbol_names},
};
use crate::{
    AssociatedItemStore, BodyOwnerData, CrateItemSource, DeclarationSourceResolver, ItemLookupEnv,
    ItemLookupIndex, ItemLookupShard, ItemResolutionQuery, ItemStore, LangItemIndex, LookupAnswer,
    LookupKeys, LookupQuestion, SemanticItemSource, SemanticItemView, SourceOwnershipIndex,
    SourceSelection, SymbolNameData, SymbolNameIndex, item::AssociatedItemHints,
};

/// Project startup reads this together to restore file routing and report macro expansion limits.
#[derive(PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct TargetDeclarationMetadata {
    /// Sources reached during module collection. Member includes are discovered by their owner's
    /// query and do not extend this startup manifest.
    pub sources: TargetSourceFiles,
    pub macro_expansion_limit: Option<MacroExpansionLimitReport>,
}

/// Files discovered while collecting a target's module declarations.
///
/// Discovery can visit a file before cfg evaluation excludes its module. `collected` retains
/// those observations for publication and graph checks; `modules` names the files that actually
/// contribute module scopes and is the preferred route for editor requests. Neither set includes
/// files that will only be discovered by lazy member expansion.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct TargetSourceFiles {
    /// Sorted unique file identities observed during collection.
    pub collected: Vec<FileId>,
    /// Sorted unique identities of files contributing module scopes.
    pub modules: Vec<FileId>,
}

/// Handles for the independently readable parts of one target's declarations.
///
/// `items` contains module declarations and trait/impl headers. Their associated declarations
/// are separate owner queries: this result keeps the inputs and macro scopes needed to build
/// them, along with cheap name hints for deciding which owners to open. Lang-item preparation
/// demands the selected owners after this query completes; other owners wait for their readers.
#[ajika::output]
pub struct TargetDeclarations {
    metadata: TargetDeclarationMetadata,
    definitions: CrateData,
    items: ItemStore,
    generated_sources: GeneratedSourceStore,
    member_inputs: AssociatedItemInputs,
    macro_scopes: MacroScopeData,
    /// Module-level identities are already allocated; member identities come from their owners.
    scope_lang_items: LangItemIndex,
    /// Owners with written lang attributes, selected without predicting their member IDs.
    lang_item_owners: Vec<ItemStoreId>,
    /// Visibility is available without preparing lang items or acquiring member declarations.
    visible_crates: UniqueVec<CrateRef>,
    lookup_shards: Vec<ajika::Output<ItemLookupShard>>,
    lookup_keys: LookupKeys,
    body_owners: Vec<BodyOwnerData>,
    symbol_names: SymbolNameIndex,
    source_ownership: SourceOwnershipIndex,
}

/// Publish module declarations and the inputs for later member queries as separate outputs.
/// Reading this result gives handles; each reader acquires the payloads it needs through its own
/// query frame, without observing unrelated member data or source maps.
#[ajika::query]
pub fn target_declarations(
    db: &QueryContext<'_>,
    krate: CrateRef,
) -> ajika::Result<TargetDeclarations> {
    let dependencies = TargetDeclarationReader::for_target(db, krate)?;
    let definitions = CrateDefMapReader(&dependencies);
    let (built, trees) = CrateData::build_target(db, krate, &DefMapQuery::new(definitions))?;
    let mut collected = trees.files().map(|tree| tree.file).collect::<Vec<_>>();
    collected.sort_unstable();
    let mut data = built.data;
    let TargetItemData {
        items,
        visible_crates,
        lang_item_owners,
        lookup,
        generated_sources,
        symbol_names,
        mut source_ownership,
    } = TargetItemData::build(
        db,
        &data,
        trees,
        built.generated_items,
        &built.member_inputs,
        &dependencies,
    )?;
    let mut body_owners = Vec::new();
    for owner in BodyOwnerData::for_crate(&items, data.def_map()) {
        db.check_cancelled()?;
        body_owners.push(owner);
    }
    data.shrink_to_fit();
    let mut module_files: Vec<_> = data
        .def_map()
        .modules()
        .iter()
        .flat_map(|module| module.origin.files())
        .collect();
    module_files.sort_unstable();
    module_files.dedup();
    for &file in &collected {
        source_ownership.record(file, ItemStoreRef::scope(DefMapRef::Crate(krate)));
    }
    let metadata = TargetDeclarationMetadata {
        sources: TargetSourceFiles {
            collected,
            modules: module_files,
        },
        macro_expansion_limit: built.macro_expansion_limit,
    };
    let scope_lang_items = items.lang_items().clone();
    let member_inputs = AssociatedItemInputs::publish(db, &items, built.member_inputs)?;
    let (lookup_keys, shards) = lookup.into_parts();
    let lookup_shards = shards
        .into_iter()
        .enumerate()
        .map(|(shard, data)| db.output(("lookup", shard as u32), data.into()))
        .collect::<ajika::Result<Vec<_>>>()?;
    TargetDeclarations::new(
        db,
        metadata,
        data,
        items,
        generated_sources,
        member_inputs,
        built.macro_scopes,
        scope_lang_items,
        lang_item_owners,
        visible_crates,
        lookup_shards,
        lookup_keys,
        body_owners,
        symbol_names,
        source_ownership,
    )
}

/// Transient construction result. The target query publishes these payloads independently so
/// searching names does not acquire lookup tables, member signatures, or generated source maps.
pub struct TargetItemData {
    pub items: ItemStore,
    pub visible_crates: UniqueVec<CrateRef>,
    pub lang_item_owners: Vec<ItemStoreId>,
    pub lookup: ItemLookupIndex,
    pub generated_sources: GeneratedSourceStore,
    pub symbol_names: SymbolNameIndex,
    pub source_ownership: SourceOwnershipIndex,
}

impl TargetItemData {
    /// Lower module declarations and owner headers, then build lookup and search hints.
    /// Member syntax is inspected for names and language attributes, without expanding calls or
    /// constructing member signatures. This is enough to select an owner for a later query.
    pub fn build(
        db: &QueryContext<'_>,
        data: &CrateData,
        trees: ItemTrees,
        generated_items: GeneratedItemStore,
        member_inputs: &HashMap<AssociatedItemOwner, AssociatedItemInput>,
        dependencies: &(impl CrateDataSource + CrateItemSource),
    ) -> ajika::Result<Self> {
        let mut items = ItemStore::lower_crate(data.def_map(), &trees, &generated_items)
            .map_err(|error| ajika::Error::application(error.into_boxed_dyn_error()))?;
        let mut lookup_hints = HashMap::new();
        let mut lang_item_owners = Vec::new();
        let mut source_ownership = SourceOwnershipIndex::default();
        let mut symbol_names = SymbolNameIndex {
            declarations: SymbolNameData::for_target(&items, data.def_map()),
            ..SymbolNameIndex::default()
        };
        let owners: Vec<_> = items
            .traits_with_refs()
            .map(|(owner, data)| {
                (
                    owner.member_store(),
                    AssociatedItemOwner::Trait(data.local_def.local_def),
                    data.source.file_id,
                )
            })
            .chain(items.impls_with_refs().map(|(owner, data)| {
                (
                    owner.member_store(),
                    AssociatedItemOwner::Impl(data.local_impl.local_impl),
                    data.source.file_id,
                )
            }))
            .collect();
        for (owner, collected, file) in owners {
            source_ownership.record(file, owner);
            let input = member_inputs
                .get(&collected)
                .ok_or_else(|| ajika::Error::contract("collected owner has no member input"))?;
            let mut hints = AssociatedItemHints::default();
            let mut written_names = Vec::new();
            let mut has_lang_items = false;
            for (_, item) in input.declarations(db, items.crate_ref())? {
                has_lang_items |= item.lang_item.is_some();
                match &item.kind {
                    ItemKind::Function(_) => hints.functions.extend(item.name.clone()),
                    ItemKind::Const(_) => hints.consts.extend(item.name.clone()),
                    ItemKind::TypeAlias(_) => hints.types.extend(item.name.clone()),
                    ItemKind::MacroCall(_) => {
                        hints.has_macros = true;
                        continue;
                    }
                    _ => continue,
                }
                // Match the name retained by semantic lowering for incomplete declarations too.
                written_names.push(item.name.clone().unwrap_or_else(Name::missing));
            }
            // The owner query will allocate final IDs and validate each attribute's target kind.
            // Keep only the owner here: preceding macro expansions can shift a member's ID.
            // TODO: Discover lang attributes hidden entirely inside unselected member macros
            // without expanding every owner. Opening unrelated stores must not change the index.
            if has_lang_items {
                lang_item_owners.push(owner.id);
            }

            // Reuse the lookup-hint walk for symbol search. Member lowering assigns the final
            // IDs, so these written names select a whole owner, not syntax-order IDs.
            if hints.has_macros {
                symbol_names.macro_owners.push(owner);
                source_ownership.defer(owner);
            } else if !written_names.is_empty() {
                symbol_names.members.push(MemberNameData {
                    owner,
                    names: written_names,
                });
            }
            lookup_hints.insert(owner.id, hints);
        }
        drop(trees);
        let generated_sources = generated_items.into_sources();

        // Read the active target locally so header resolution never asks for its unpublished
        // outputs. Only completed dependencies go through the supplied readers.
        let local = LocalDeclarations {
            dependencies,
            definitions: data,
            items: &items,
        };
        let definitions = CrateDefMapReader(local);
        let headers =
            impl_header_resolutions(&items, &ItemResolutionQuery::new(definitions, local))?;
        let visible_crates =
            DefMapQuery::new(definitions).item_lookup_crates_from(items.crate_ref())?;

        let mut self_heads = HashMap::new();
        for header in headers {
            items.set_impl_header_facts(
                header.impl_ref.id,
                header.resolved_self_ty,
                header.resolved_trait_ref,
            );
            if let Some(head) = header.self_head {
                self_heads.insert(header.impl_ref, head);
            }
        }
        let mut lookup = ItemLookupIndex::build_from_headers(&items, &self_heads, lookup_hints);
        items.shrink_to_fit();
        lookup.shrink_to_fit();
        symbol_names.shrink_to_fit();
        lang_item_owners.shrink_to_fit();
        Ok(Self {
            items,
            visible_crates,
            lang_item_owners,
            lookup,
            generated_sources,
            symbol_names,
            source_ownership,
        })
    }
}

#[ajika::query(transient)]
fn visible_targets(db: &QueryContext<'_>, root: CrateRef) -> ajika::Result<HashSet<CrateRef>> {
    let mut reached = HashSet::new();
    let mut pending = vec![root];
    while let Some(krate) = pending.pop() {
        if reached.insert(krate) {
            let roots = CrateDependencies::at(krate).roots(db)?;
            pending.extend(
                roots
                    .values()
                    .rev()
                    .map(|module| module.origin.origin_crate()),
            );
        }
    }
    Ok(reached)
}

/// Restricts declaration reads to the targets in this view. Ajika retains the payloads for
/// the consuming query frame.
pub struct TargetDeclarationReader<'db> {
    db: &'db QueryContext<'db>,
    targets: Arc<HashSet<CrateRef>>,
}

impl<'db> TargetDeclarationReader<'db> {
    /// The host has already accepted the complete target set for this operation.
    pub fn for_project(
        db: &'db QueryContext<'db>,
        targets: impl IntoIterator<Item = CrateRef>,
    ) -> Self {
        Self {
            db,
            targets: Arc::new(targets.into_iter().collect()),
        }
    }

    /// Read declarations for one target and the dependencies visible to it.
    pub fn for_target(db: &'db QueryContext<'db>, krate: CrateRef) -> ajika::Result<Self> {
        let targets = visible_targets(db, krate)?;
        Ok(Self { db, targets })
    }

    #[cfg(test)]
    pub(crate) fn lookup_entry_count(&self, krate: CrateRef) -> usize {
        let declarations = target_declarations(self.db, krate).expect("fixture declarations");
        declarations
            .lookup_shards
            .value_ref(self.db)
            .expect("fixture lookup shards")
            .iter()
            .map(|shard| {
                shard
                    .value_ref(self.db)
                    .expect("fixture lookup shard")
                    .entry_count()
            })
            .sum()
    }

    pub fn targets(&self) -> impl Iterator<Item = CrateRef> + '_ {
        self.targets.iter().copied()
    }

    fn acquire<T: ajika::Persist + PartialEq + Send + Sync + 'static>(
        &self,
        krate: CrateRef,
        output: impl FnOnce(&TargetDeclarations) -> &ajika::Output<T>,
    ) -> ajika::Result<Option<&'db T>> {
        if !self.targets.contains(&krate) {
            return Ok(None);
        }
        let declarations = target_declarations(self.db, krate)?;
        output(&declarations).value_ref(self.db).map(Some)
    }
}

impl fmt::Debug for TargetDeclarationReader<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TargetDeclarationReader")
            .field("targets", &self.targets.len())
            .finish_non_exhaustive()
    }
}

impl CrateDataSource for TargetDeclarationReader<'_> {
    fn crate_data(&self, krate: CrateRef) -> Result<Option<&CrateData>, ReadError> {
        self.acquire(krate, |result| &result.definitions)
            .map_err(ReadError::Query)
    }
}

impl CrateItemSource for TargetDeclarationReader<'_> {
    fn associated_items(
        &self,
        store: ItemStoreRef,
    ) -> Result<Option<&AssociatedItemStore>, ReadError> {
        let Some(krate) = store.origin.as_crate_ref() else {
            return Ok(None);
        };
        if !self.targets.contains(&krate) || store.id == ItemStoreId::Scope {
            return Ok(None);
        }
        let Some(members) =
            associated_declarations(self.db, krate, store.id).map_err(ReadError::Query)?
        else {
            return Ok(None);
        };
        members
            .items
            .value_ref(self.db)
            .map(Some)
            .map_err(ReadError::Query)
    }

    fn item(&self, item: SemanticItemRef) -> Result<Option<SemanticItemView<'_>>, ReadError> {
        let Some(krate) = item.origin().as_crate_ref() else {
            return Ok(None);
        };
        let store = item.store();
        if store.id != ItemStoreId::Scope {
            return Ok(self
                .associated_items(store)?
                .and_then(|members| members.item(item)));
        }
        let Some(items) = self
            .acquire(krate, |result| &result.items)
            .map_err(ReadError::Query)?
        else {
            return Ok(None);
        };
        Ok(items.semantic_item_view(item))
    }

    fn item_for_local_def(
        &self,
        definition: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, ReadError> {
        let Some(krate) = definition.origin.as_crate_ref() else {
            return Ok(None);
        };
        let Some(items) = self
            .acquire(krate, |result| &result.items)
            .map_err(ReadError::Query)?
        else {
            return Ok(None);
        };
        Ok(items
            .item_for_local_def(definition.local_def)
            .map(|item| item.semantic_ref(definition.origin)))
    }

    fn scope_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'_>>, ReadError> {
        let Some(krate) = origin.as_crate_ref() else {
            return Ok(Vec::new());
        };
        Ok(self
            .acquire(krate, |result| &result.items)
            .map_err(ReadError::Query)?
            .map(|items| items.scope_items().collect())
            .unwrap_or_default())
    }

    fn all_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'_>>, ReadError> {
        let Some(krate) = origin.as_crate_ref() else {
            return Ok(Vec::new());
        };
        let Some(items) = self
            .acquire(krate, |result| &result.items)
            .map_err(ReadError::Query)?
        else {
            return Ok(Vec::new());
        };
        let mut result: Vec<_> = items.scope_items().collect();
        for owner in items.member_stores() {
            if let Some(members) = self.associated_items(owner)? {
                result.extend(members.semantic_items(owner));
            }
        }
        Ok(result)
    }

    fn source_items(
        &self,
        origin: DefMapRef,
        selection: SourceSelection,
    ) -> Result<Vec<SemanticItemView<'_>>, ReadError> {
        let Some(krate) = origin.as_crate_ref() else {
            return Ok(Vec::new());
        };
        let Some(items) = self
            .acquire(krate, |result| &result.items)
            .map_err(ReadError::Query)?
        else {
            return Ok(Vec::new());
        };
        let mut selected: Vec<_> = items
            .scope_items()
            .filter(|item| {
                selection
                    .file()
                    .is_none_or(|file| item.source().file_id == file)
            })
            .collect();
        for owner in DeclarationSourceResolver::new(self.db, krate)
            .member_stores(selection)
            .map_err(ReadError::Query)?
        {
            if let Some(members) = self.associated_items(owner)? {
                selected.extend(members.semantic_items(owner));
            }
        }
        Ok(selected)
    }

    fn lookup_env(&self, krate: CrateRef) -> Result<Option<&ItemLookupEnv>, ReadError> {
        if !self.targets.contains(&krate) {
            return Ok(None);
        }
        ItemLookupEnv::for_crate(self.db, krate)
            .map(Some)
            .map_err(ReadError::Query)
    }

    fn lookup_candidates(
        &self,
        use_site: CrateRef,
        question: &LookupQuestion,
    ) -> Result<LookupAnswer, ReadError> {
        super::lookup::lookup_candidates(self.db, use_site, question.clone())
            .cloned()
            .map_err(ReadError::Query)
    }

    fn symbol_names(&self, crates: &[CrateRef]) -> Result<Vec<Arc<SymbolNameIndex>>, ReadError> {
        let targets: Vec<_> = crates
            .iter()
            .copied()
            .filter(|krate| self.targets.contains(krate))
            .collect();
        // Broad search must learn names from every macro-bearing owner. Keep that cached discovery
        // separate from startup's written-name table, so subsequent searches can share both tables
        // without copying them or reopening ordinary member stores.
        let tables = self
            .db
            .parallel_map(&targets, |db, &krate| {
                let written = target_declarations(db, krate)?.symbol_names.value(db)?;
                let generated = db.query::<macro_symbol_names>(krate)?;
                Ok([written, generated])
            })
            .map_err(ReadError::Query)?;
        Ok(tables.into_iter().flatten().collect())
    }

    fn crates(&self) -> Vec<CrateRef> {
        let mut targets: Vec<_> = self.targets().collect();
        targets.sort_unstable_by_key(|krate| (krate.package.0, krate.crate_id.0));
        targets
    }
}

#[derive(Clone, Copy)]
struct LocalDeclarations<'a, D> {
    dependencies: D,
    definitions: &'a CrateData,
    items: &'a ItemStore,
}

impl<D: CrateDataSource> CrateDataSource for LocalDeclarations<'_, D> {
    fn crate_data(&self, krate: CrateRef) -> Result<Option<&CrateData>, ReadError> {
        if krate == self.items.crate_ref() {
            return Ok(Some(self.definitions));
        }
        self.dependencies.crate_data(krate)
    }
}

impl<'a, D: CrateItemSource> SemanticItemSource<'a> for LocalDeclarations<'a, &'a D> {
    fn associated_items(
        &self,
        store: ItemStoreRef,
    ) -> Result<Option<&'a AssociatedItemStore>, ReadError> {
        if store.origin == self.items.origin() {
            return Err(ReadError::Query(ajika::Error::contract(
                "member declarations requested while building headers",
            )));
        }
        self.dependencies.associated_items(store)
    }

    fn item(&self, item: SemanticItemRef) -> Result<Option<SemanticItemView<'a>>, ReadError> {
        if item.origin() == self.items.origin() {
            return Ok(self.items.semantic_item_view(item));
        }
        self.dependencies.item(item)
    }

    fn item_for_local_def(
        &self,
        definition: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, ReadError> {
        if definition.origin == self.items.origin() {
            return Ok(self
                .items
                .item_for_local_def(definition.local_def)
                .map(|item| item.semantic_ref(definition.origin)));
        }
        self.dependencies.item_for_local_def(definition)
    }

    fn scope_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'a>>, ReadError> {
        if origin == self.items.origin() {
            return Ok(self.items.scope_items().collect());
        }
        self.dependencies.scope_items(origin)
    }

    fn all_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'a>>, ReadError> {
        if origin == self.items.origin() {
            return Err(ReadError::Query(ajika::Error::contract(
                "all declarations requested while building headers",
            )));
        }
        self.dependencies.all_items(origin)
    }

    fn source_items(
        &self,
        origin: DefMapRef,
        selection: SourceSelection,
    ) -> Result<Vec<SemanticItemView<'a>>, ReadError> {
        if origin == self.items.origin() {
            return Err(ReadError::Query(ajika::Error::contract(
                "source scans requested while building headers",
            )));
        }
        self.dependencies.source_items(origin, selection)
    }
}
