use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
    time::Duration,
};

use rg_def_map::{CrateData, CrateDataSource, CrateDefMapReader, CrateDependencies, DefMapQuery};
use rg_ir_model::{BodyKey, CrateRef, DefMapRef, LocalDefRef, SemanticItemRef};
use rg_item_tree::{FileTree, ItemTrees, file_declarations, target_sources};
use rg_parse::{ParseDb, TargetFileManifest, saved_syntax};
use rg_semantic_ir::{
    CrateItemSource, ItemLookupIndex, ItemStore, LangItemIndex, SemanticItemView,
    target_declarations,
};
use rg_source::{
    QueryContext, QueryDatabase, QueryServices, ReadError, SourceInput, SourcePresence,
};
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
        let inputs = ajika::Inputs::default()
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
                update.forget_all(SourceInput::CAPTURE);
                update.forget_all(SourcePresence::RESOLVED_FILE);
                Ok(())
            })
            .expect("benchmark source inputs should reset");
        QueryDatabase::memory(
            inputs,
            QueryServices::default(),
            ajika::Retention {
                max_entries: usize::MAX,
                max_bytes: u64::MAX,
                max_idle: Duration::MAX,
            },
        )
    }

    pub(crate) fn parse_roots(
        &self,
        db: &QueryContext<'_>,
        inputs: &ajika::Inputs,
    ) -> ajika::Result<ParseDb> {
        let mut manifests = Vec::with_capacity(self.targets.len());
        for &krate in &self.targets {
            let path = TargetInput::at(krate).root_path(db)?;
            let file = SourcePresence::at(path)
                .resolved_file(db)?
                .expect("benchmark target root should exist");
            let edition = PackageInput::at(krate.package).edition(db)?;
            saved_syntax(db, file, edition)?;
            manifests.push(TargetFileManifest {
                root: file,
                files: vec![file],
            });
        }
        let parse = ParseDb::from_target_manifests(
            &self.workspace,
            inputs,
            self.targets.iter().copied().zip(&manifests),
        )?;
        // The old root parse included line indexes. Build them explicitly now that routing
        // leaves them lazy.
        for file in parse.packages().iter().flat_map(|pkg| pkg.parsed_files()) {
            file.line_index()
                .map_err(|error| ajika::Error::application(error.into_boxed_dyn_error()))?;
        }
        Ok(parse)
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
            file_declarations(db, package, file)
        })
    }

    pub(crate) fn scopes(&self, db: &QueryContext<'_>) -> ajika::Result<PreparedScopes> {
        let mut scopes = PreparedScopes::default();
        // The target graph supplies dependency-first order. Each target can settle its scopes
        // against completed definitions without demanding the dependency's semantic items.
        for &krate in &self.targets {
            let dependencies = DefMapQuery::new(CrateDefMapReader(&scopes.declarations));
            let (built, trees, _) = CrateData::build_target(db, krate, &dependencies)?;
            scopes.declarations.definitions.insert(krate, built.data);
            scopes.targets.push((krate, trees, built.generated_items));
        }
        Ok(scopes)
    }

    pub(crate) fn body_owners(&self, db: &QueryContext<'_>) -> ajika::Result<Vec<BodyKey>> {
        let declarations =
            db.parallel_map(&self.targets, |db, &krate| target_declarations(db, krate))?;
        let mut owners = Vec::new();
        for (&krate, declarations) in self.targets.iter().zip(&declarations) {
            if self.workspace.packages()[krate.package.0].is_workspace_member
                && TargetInput::at(krate)
                    .kind(db)?
                    .is_primary_analysis_target()
            {
                owners.extend(
                    declarations
                        .body_owners
                        .value(db)?
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
    targets: Vec<(CrateRef, ItemTrees, rg_def_map::GeneratedItemStore)>,
}

impl PreparedScopes {
    pub(crate) fn build_items(mut self, db: &QueryContext<'_>) -> ajika::Result<Declarations> {
        for (krate, trees, generated) in self.targets {
            let items = ItemStore::build_target(
                db,
                &self.declarations.definitions[&krate],
                trees,
                generated,
                &self.declarations,
            )?;
            self.declarations.items.insert(krate, items);
        }
        Ok(self.declarations)
    }
}

#[derive(Default)]
pub(crate) struct Declarations {
    definitions: HashMap<CrateRef, CrateData>,
    items: HashMap<CrateRef, (ItemStore, ItemLookupIndex)>,
}

impl CrateDataSource for Declarations {
    type Error = ReadError;

    fn crate_data(&self, krate: CrateRef) -> Result<Option<&CrateData>, ReadError> {
        Ok(self.definitions.get(&krate))
    }
}

impl CrateItemSource for Declarations {
    fn item(&self, item: SemanticItemRef) -> Result<Option<SemanticItemView<'_>>, ReadError> {
        Ok(item
            .origin()
            .as_crate_ref()
            .and_then(|krate| self.items.get(&krate).map(|(items, _)| items))
            .and_then(|items| items.semantic_item_view(item)))
    }

    fn item_for_local_def(
        &self,
        definition: LocalDefRef,
    ) -> Result<Option<SemanticItemRef>, ReadError> {
        Ok(definition
            .origin
            .as_crate_ref()
            .and_then(|krate| self.items.get(&krate).map(|(items, _)| items))
            .and_then(|items| {
                items
                    .item_for_local_def(definition.local_def)
                    .map(|item| item.semantic_ref(definition.origin))
            }))
    }

    fn all_items(&self, origin: DefMapRef) -> Result<Vec<SemanticItemView<'_>>, ReadError> {
        Ok(origin
            .as_crate_ref()
            .and_then(|krate| self.items.get(&krate).map(|(items, _)| items))
            .map(|items| items.semantic_items().collect())
            .unwrap_or_default())
    }

    fn lang_items(&self, krate: CrateRef) -> Result<Option<&LangItemIndex>, ReadError> {
        Ok(self.items.get(&krate).map(|(items, _)| items.lang_items()))
    }

    fn lookup_index(&self, krate: CrateRef) -> Result<Option<&ItemLookupIndex>, ReadError> {
        Ok(self.items.get(&krate).map(|(_, index)| index))
    }

    fn crates(&self) -> Vec<CrateRef> {
        self.items.keys().copied().collect()
    }
}
