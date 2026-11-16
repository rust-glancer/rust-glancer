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
use rg_parse::TargetFileManifest;
use rg_source::{QueryContext, ReadError};
use rg_std::Shrink as _;
use wincode::{SchemaRead, SchemaWrite};

use super::{
    impl_headers::impl_header_resolutions,
    members::{AssociatedItemInputs, associated_declarations},
};
use crate::{
    AssociatedItemStore, BodyOwnerData, CrateItemSource, ItemLookupEnv, ItemLookupIndex,
    ItemLookupShard, ItemReader, ItemResolutionQuery, ItemStore, LangItemIndex, LookupAnswer,
    LookupKeys, LookupQuestion, SavedSourceResolver, SemanticItemSource, SemanticItemView,
    SourceOwnershipIndex, SourceSelection, SymbolNameData,
    item::AssociatedItemHints,
};

/// Project startup reads this together to restore file routing and report macro expansion limits.
#[derive(PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct TargetDeclarationMetadata {
    /// Sources reached during module collection. Member includes are discovered by their owner's
    /// query and do not extend this startup manifest.
    pub sources: TargetFileManifest,
    pub module_files: Vec<FileId>,
    pub macro_expansion_limit: Option<MacroExpansionLimitReport>,
}

/// Handles for the independently readable parts of one target's declarations.
///
/// `items` contains module declarations and trait/impl headers. Their associated declarations
/// are separate owner queries: this result keeps the inputs and macro scopes needed to build
/// them, along with cheap name hints for deciding which owners to open. Those member queries
/// are left for readers that need their declarations or include files.
#[ajika::output]
pub struct TargetDeclarations {
    metadata: TargetDeclarationMetadata,
    definitions: CrateData,
    items: ItemStore,
    generated_sources: GeneratedSourceStore,
    member_inputs: AssociatedItemInputs,
    macro_scopes: MacroScopeData,
    lang_items: LangItemIndex,
    lookup_env: ItemLookupEnv,
    lookup_shards: Vec<ajika::Output<ItemLookupShard>>,
    lookup_keys: LookupKeys,
    body_owners: Vec<BodyOwnerData>,
    symbol_names: Vec<SymbolNameData>,
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
    let (built, trees, sources) =
        CrateData::build_target(db, krate, &DefMapQuery::new(definitions))?;
    let mut data = built.data;
    let TargetItemData {
        items,
        lookup_env,
        lookup,
        generated_sources,
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
    for &file in &sources.files {
        source_ownership.record(file, ItemStoreRef::scope(DefMapRef::Crate(krate)));
    }
    let metadata = TargetDeclarationMetadata {
        sources,
        module_files,
        macro_expansion_limit: built.macro_expansion_limit,
    };
    let lang_items = items.lang_items().clone();
    let member_inputs = AssociatedItemInputs::publish(db, &items, built.member_inputs)?;
    let symbol_names = SymbolNameData::for_target(&items, data.def_map());
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
        lang_items,
        lookup_env,
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
    pub lookup_env: ItemLookupEnv,
    pub lookup: ItemLookupIndex,
    pub generated_sources: GeneratedSourceStore,
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
        let mut source_ownership = SourceOwnershipIndex::default();
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
            for (source, item) in input.declarations(db, items.crate_ref())? {
                if let Some(lang_item) = item.lang_item {
                    items
                        .lang_items
                        .insert_member(lang_item, owner, source, &item.kind);
                }
                match &item.kind {
                    ItemKind::Function(_) => hints.functions.extend(item.name.clone()),
                    ItemKind::Const(_) => hints.consts.extend(item.name.clone()),
                    ItemKind::TypeAlias(_) => hints.has_types = true,
                    ItemKind::MacroCall(_) => {
                        hints.has_macros = true;
                        continue;
                    }
                    _ => continue,
                }
            }
            if hints.has_macros {
                source_ownership.defer(owner);
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
        let env = ItemLookupEnv::build(visible_crates, &items, ItemReader(dependencies), || {
            db.check_cancelled().map_err(ReadError::Query)
        })?;

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
        Ok(Self {
            items,
            lookup_env: env,
            lookup,
            generated_sources,
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
        for owner in SavedSourceResolver::new(self.db, krate)
            .member_stores(selection)
            .map_err(ReadError::Query)?
        {
            if let Some(members) = self.associated_items(owner)? {
                selected.extend(members.semantic_items(owner));
            }
        }
        Ok(selected)
    }

    fn lang_items(&self, krate: CrateRef) -> Result<Option<&LangItemIndex>, ReadError> {
        self.acquire(krate, |result| &result.lang_items)
            .map_err(ReadError::Query)
    }

    fn lookup_env(&self, krate: CrateRef) -> Result<Option<&ItemLookupEnv>, ReadError> {
        self.acquire(krate, |result| &result.lookup_env)
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

    fn symbol_names(&self, krate: CrateRef) -> Result<&[SymbolNameData], ReadError> {
        Ok(self
            .acquire(krate, |result| &result.symbol_names)
            .map_err(ReadError::Query)?
            .map_or(&[], Vec::as_slice))
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
