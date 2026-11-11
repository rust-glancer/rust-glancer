//! Publish one configured target's independently readable semantic payloads.

use std::{
    collections::{HashMap, HashSet},
    fmt,
};

use rg_def_map::{
    CrateData, CrateDataSource, CrateDefMapReader, CrateDependencies, DefMapQuery,
    GeneratedItemStore, MacroExpansionLimitReport,
};
use rg_ir_model::{CrateRef, DefMapRef, FileId};
use rg_item_tree::ItemTrees;
use rg_parse::TargetFileManifest;
use rg_source::ReadError;
use rg_std::{OperationError, Shrink as _};
use wincode::{SchemaRead, SchemaWrite};

use super::impl_headers::impl_header_resolutions;
use crate::{
    BodyOwnerData, CrateItemSource, ItemLookupFacts, ItemLookupIndex, ItemReader,
    ItemResolutionQuery, ItemStore, ItemStoreSource, LangItemIndex,
};

/// Project startup reads this together to restore file routing and report macro expansion limits.
#[derive(PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct TargetDeclarationMetadata {
    pub sources: TargetFileManifest,
    pub module_files: Vec<FileId>,
    pub macro_expansion_limit: Option<MacroExpansionLimitReport>,
}

/// Handles for the independently readable parts of one target's declarations.
#[ajika::output]
pub struct TargetDeclarations {
    metadata: TargetDeclarationMetadata,
    definitions: CrateData,
    items: ItemStore,
    lang_items: LangItemIndex,
    lookup: ItemLookupIndex,
    body_owners: Vec<BodyOwnerData>,
}

#[ajika::query]
pub fn target_declarations(
    db: &rg_source::QueryContext<'_>,
    krate: CrateRef,
) -> ajika::Result<TargetDeclarations> {
    let dependencies = TargetDeclarationReader::for_target(db, krate)?;
    let definitions = CrateDefMapReader(&dependencies);
    let (built, trees, sources) =
        CrateData::build_target(db, krate, &DefMapQuery::new(definitions))?;
    let mut data = built.data;
    let (items, lookup) =
        ItemStore::build_target(db, &data, trees, built.generated_items, &dependencies)?;
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
    let metadata = TargetDeclarationMetadata {
        sources,
        module_files,
        macro_expansion_limit: built.macro_expansion_limit,
    };
    let lang_items = items.lang_items().clone();
    TargetDeclarations::new(db, metadata, data, items, lang_items, lookup, body_owners)
}

impl ItemStore {
    /// Lower a target's settled scopes into semantic items and build their lookup index.
    pub fn build_target(
        db: &rg_source::QueryContext<'_>,
        data: &CrateData,
        trees: ItemTrees,
        generated_items: GeneratedItemStore,
        dependencies: &(impl CrateDataSource<Error = ReadError> + CrateItemSource),
    ) -> ajika::Result<(Self, ItemLookupIndex)> {
        let mut items = Self::lower_crate(data.def_map(), &trees, &generated_items)
            .map_err(|error| ajika::Error::application(error.into_boxed_dyn_error()))?;
        drop(trees);
        drop(generated_items);

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
        let facts =
            ItemLookupFacts::build(visible_crates, &items, ItemReader(dependencies), || {
                db.check_cancelled()
                    .map_err(|error| OperationError::Source(ReadError::Query(error)))
            })
            .map_err(ReadError::from)?;

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
        let mut lookup = ItemLookupIndex::build_from_store(&items, &self_heads, facts);
        items.shrink_to_fit();
        lookup.shrink_to_fit();
        Ok((items, lookup))
    }
}

#[ajika::query(transient)]
fn visible_targets(
    db: &rg_source::QueryContext<'_>,
    root: CrateRef,
) -> ajika::Result<Vec<CrateRef>> {
    let mut reached = HashSet::new();
    let mut pending = vec![root];
    let mut targets = Vec::new();
    while let Some(krate) = pending.pop() {
        if reached.insert(krate) {
            targets.push(krate);
            let roots = CrateDependencies::at(krate).roots(db)?;
            pending.extend(
                roots
                    .values()
                    .rev()
                    .map(|module| module.origin.origin_crate()),
            );
        }
    }
    Ok(targets)
}

/// Restricts declaration reads to the targets in this view. Ajika retains the payloads for
/// the consuming query frame.
pub struct TargetDeclarationReader<'db> {
    db: &'db rg_source::QueryContext<'db>,
    targets: HashSet<CrateRef>,
}

impl<'db> TargetDeclarationReader<'db> {
    /// The host has already accepted the complete target directory for this operation.
    pub fn for_project(
        db: &'db rg_source::QueryContext<'db>,
        targets: impl IntoIterator<Item = CrateRef>,
    ) -> Self {
        Self {
            db,
            targets: targets.into_iter().collect(),
        }
    }

    /// Read declarations for one target and the dependencies visible to it.
    pub fn for_target(
        db: &'db rg_source::QueryContext<'db>,
        krate: CrateRef,
    ) -> ajika::Result<Self> {
        let targets = visible_targets(db, krate)?;
        Ok(Self {
            db,
            targets: targets.iter().copied().collect(),
        })
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
    type Error = ReadError;

    fn crate_data(&self, krate: CrateRef) -> Result<Option<&CrateData>, ReadError> {
        self.acquire(krate, |result| &result.definitions)
            .map_err(ReadError::Query)
    }
}

impl CrateItemSource for TargetDeclarationReader<'_> {
    fn items(&self, krate: CrateRef) -> Result<Option<&ItemStore>, ReadError> {
        self.acquire(krate, |result| &result.items)
            .map_err(ReadError::Query)
    }

    fn lang_items(&self, krate: CrateRef) -> Result<Option<&LangItemIndex>, ReadError> {
        self.acquire(krate, |result| &result.lang_items)
            .map_err(ReadError::Query)
    }

    fn lookup_index(&self, krate: CrateRef) -> Result<Option<&ItemLookupIndex>, ReadError> {
        self.acquire(krate, |result| &result.lookup)
            .map_err(ReadError::Query)
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

impl<D: CrateDataSource<Error = ReadError>> CrateDataSource for LocalDeclarations<'_, D> {
    type Error = ReadError;

    fn crate_data(&self, krate: CrateRef) -> Result<Option<&CrateData>, ReadError> {
        if krate == self.items.crate_ref() {
            return Ok(Some(self.definitions));
        }
        self.dependencies.crate_data(krate)
    }
}

impl<'a, D: CrateItemSource> ItemStoreSource<'a> for LocalDeclarations<'a, &'a D> {
    type Error = ReadError;

    fn item_store_for_origin(&self, origin: DefMapRef) -> Result<Option<&'a ItemStore>, ReadError> {
        if origin == self.items.origin() {
            return Ok(Some(self.items));
        }
        match origin.as_crate_ref() {
            Some(krate) => self.dependencies.items(krate),
            None => Ok(None),
        }
    }
}
