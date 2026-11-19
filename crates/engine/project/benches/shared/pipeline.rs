use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
    sync::Arc,
    time::Duration,
};

use rg_def_map::{
    AssociatedItemInput, AssociatedItemOwner, CrateData, CrateDefMapReader, CrateDefinitionSource,
    CrateDependencies, CrateMetadata, DefMapQuery, DefMapView, GeneratedItemStore,
};
use rg_ir_model::{BodyKey, CrateRef, DefMapRef, ItemStoreRef, LocalDefRef, SemanticItemRef};
use rg_item_tree::{FileTree, ItemTrees, file_declarations, target_sources};
use rg_parse::{SourceReader, source_syntax};
use rg_semantic_ir::{
    AssociatedItemStore, CrateItemSource, ItemLookupEnv, LookupAnswer, LookupQuestion,
    SemanticItemView, SourceSelection, SymbolNameIndex, TargetItemData, body_owners,
    target_declarations,
};
use rg_source::{QueryContext, QueryDatabase, QueryServices, ReadError, SourceInput};
use rg_workspace::{PackageInput, TargetInput, WorkspaceMetadata};

use super::BenchTarget;

pub(crate) struct BenchWorkspace {
    workspace: WorkspaceMetadata,
    inputs: ajika::Inputs,
    targets: Vec<CrateRef>,
}

impl BenchWorkspace {
    pub(crate) fn load(target: BenchTarget) -> Self {
        let workspace = target.workspace();
        let mut targets = Vec::new();
        let inputs = SourceInput::snapshot(workspace.source_durability_roots())
            .edit(|update| {
                workspace.write_inputs(update)?;
                targets = CrateDependencies::write_inputs(&workspace, update)?;
                Ok(())
            })
            .expect("benchmark workspace inputs should install");
        Self {
            workspace,
            inputs,
            targets,
        }
    }

    pub(crate) fn database(&self) -> QueryDatabase {
        // Keep the Cargo settings but give each sample its own lazy source observations.
        let inputs = self
            .inputs
            .edit(|update| {
                SourceInput::forget_observations(update);
                Ok(())
            })
            .expect("benchmark source inputs should reset");
        QueryDatabase::memory(
            inputs,
            QueryServices::default(),
            ajika::DatabaseOptions::new(ajika::Retention {
                max_entries: usize::MAX,
                max_bytes: u64::MAX,
                max_idle: Duration::MAX,
            }),
        )
    }

    pub(crate) fn parse_roots(&self, db: &QueryContext<'_>) -> ajika::Result<()> {
        let sources = SourceReader::new(db);
        for &krate in &self.targets {
            let path = TargetInput::at(krate)
                .root_path(db)?
                .into_os_string()
                .expect("benchmark target path is native");
            let file = SourceInput::file_at(db, Path::new(&path))?
                .expect("benchmark target root should exist");
            let edition = PackageInput::at(krate.package).edition(db)?;
            source_syntax(db, file, edition)?;
            sources.line_index(file)?;
        }
        Ok(())
    }

    pub(crate) fn item_trees(&self, db: &QueryContext<'_>) -> ajika::Result<Vec<Arc<FileTree>>> {
        let sources = db.parallel_map(&self.targets, |db, &krate| target_sources(db, krate))?;
        // Sibling targets can share a file tree. Time one lowering per package and file.
        let files: BTreeSet<_> = self
            .targets
            .iter()
            .zip(&sources)
            .flat_map(|(krate, sources)| sources.files.iter().map(|&file| (krate.package, file)))
            .collect();
        let files: Vec<_> = files.into_iter().collect();
        db.parallel_map(&files, |db, &(package, file)| {
            file_declarations(db, package, file)?.items.value(db)
        })
    }

    pub(crate) fn scopes(&self, db: &QueryContext<'_>) -> ajika::Result<PreparedScopes> {
        let mut scopes = PreparedScopes::default();
        // The target graph supplies dependency-first order. Each target can settle its scopes
        // against completed definitions without demanding the dependency's semantic items.
        for &krate in &self.targets {
            let dependencies = DefMapQuery::new(CrateDefMapReader(&scopes.declarations));
            let (built, trees) = CrateData::build_target(db, krate, &dependencies)?;
            scopes.declarations.definitions.insert(krate, built.data);
            scopes.targets.push(PreparedTarget {
                krate,
                trees,
                generated: built.generated_items,
                member_inputs: built.member_inputs,
            });
        }
        Ok(scopes)
    }

    pub(crate) fn body_owners(&self, db: &QueryContext<'_>) -> ajika::Result<Vec<BodyKey>> {
        db.parallel_map(&self.targets, |db, &krate| target_declarations(db, krate))?;
        let mut owners = Vec::new();
        for &krate in &self.targets {
            if self.workspace.packages()[krate.package.0].is_workspace_member
                && TargetInput::at(krate)
                    .kind(db)?
                    .is_primary_analysis_target()
            {
                owners.extend(
                    body_owners(db, krate, SourceSelection::All)?
                        .iter()
                        .map(|owner| BodyKey {
                            krate,
                            owner: owner.owner,
                        }),
                );
            }
        }
        Ok(owners)
    }
}

#[derive(Default)]
pub(crate) struct PreparedScopes {
    declarations: Declarations,
    targets: Vec<PreparedTarget>,
}

impl PreparedScopes {
    pub(crate) fn build_items(mut self, db: &QueryContext<'_>) -> ajika::Result<Declarations> {
        for target in self.targets {
            let items = TargetItemData::build(
                db,
                &self.declarations.definitions[&target.krate],
                target.trees,
                target.generated,
                &target.member_inputs,
                &self.declarations,
            )?;
            self.declarations.items.insert(target.krate, items);
        }
        Ok(self.declarations)
    }
}

struct PreparedTarget {
    krate: CrateRef,
    trees: ItemTrees,
    generated: GeneratedItemStore,
    member_inputs: HashMap<AssociatedItemOwner, AssociatedItemInput>,
}

#[derive(Default)]
pub(crate) struct Declarations {
    definitions: HashMap<CrateRef, CrateData>,
    items: HashMap<CrateRef, TargetItemData>,
}

impl CrateDefinitionSource for Declarations {
    fn crate_metadata(&self, krate: CrateRef) -> Result<Option<&CrateMetadata>, ReadError> {
        Ok(self.definitions.get(&krate).map(CrateData::metadata))
    }
    fn definitions(&self, krate: CrateRef) -> Result<Option<DefMapView<'_>>, ReadError> {
        Ok(self
            .definitions
            .get(&krate)
            .map(|data| data.def_map().view()))
    }
}

impl CrateItemSource for Declarations {
    fn associated_items(
        &self,
        store: ItemStoreRef,
    ) -> Result<Option<&AssociatedItemStore>, ReadError> {
        let _ = store;
        Err(ReadError::Query(ajika::Error::contract(
            "member reads are outside the header benchmark",
        )))
    }

    fn item(&self, item: SemanticItemRef) -> Result<Option<SemanticItemView<'_>>, ReadError> {
        Ok(item
            .origin()
            .as_crate_ref()
            .and_then(|krate| self.items.get(&krate).map(|data| &data.items))
            .and_then(|items| items.semantic_item_view(item)))
    }

    fn item_for_local_def(
        &self,
        definition: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, ReadError> {
        Ok(definition
            .origin
            .as_crate_ref()
            .and_then(|krate| self.items.get(&krate).map(|data| &data.items))
            .and_then(|items| {
                items
                    .item_for_local_def(definition.local_def)
                    .map(|item| item.semantic_ref(definition.origin))
            }))
    }

    fn scope_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'_>>, ReadError> {
        Ok(origin
            .as_crate_ref()
            .and_then(|krate| self.items.get(&krate).map(|data| &data.items))
            .map(|items| items.scope_items().collect())
            .unwrap_or_default())
    }

    fn all_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'_>>, ReadError> {
        let _ = origin;
        Err(ReadError::Query(ajika::Error::contract(
            "broad reads are outside the header benchmark",
        )))
    }

    fn source_items(
        &self,
        origin: DefMapRef,
        selection: SourceSelection,
    ) -> Result<Vec<SemanticItemView<'_>>, ReadError> {
        let _ = (origin, selection);
        Err(ReadError::Query(ajika::Error::contract(
            "source scans are outside the header benchmark",
        )))
    }

    fn lookup_env(&self, _krate: CrateRef) -> Result<Option<&ItemLookupEnv>, ReadError> {
        Err(ReadError::Query(ajika::Error::contract(
            "lookup preparation is outside the header benchmark",
        )))
    }

    fn lookup_candidates(
        &self,
        use_site: CrateRef,
        question: &LookupQuestion,
    ) -> Result<LookupAnswer, ReadError> {
        let indexes = self.items[&use_site]
            .visible_crates
            .iter()
            .filter_map(|krate| self.items.get(krate).map(|data| &data.lookup));
        question.answer(
            indexes.flat_map(|index| index.shards_for(question)),
            |owner| self.associated_items(owner),
        )
    }

    fn symbol_names(&self, _crates: &[CrateRef]) -> Result<Vec<Arc<SymbolNameIndex>>, ReadError> {
        // The isolated declaration-lowering benchmark does not run symbol discovery.
        Ok(Vec::new())
    }

    fn crates(&self) -> Vec<CrateRef> {
        self.items.keys().copied().collect()
    }
}
