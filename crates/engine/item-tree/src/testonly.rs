use std::time::Duration;
#[cfg(test)]
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(test)]
use rg_ir_model::{FileId, PackageSlot, Span};
#[cfg(test)]
use rg_workspace::WorkspaceLoweringConfig;
use rg_workspace::WorkspaceMetadata;
#[cfg(test)]
use test_fixture::{CrateFixture, fixture_crate};

#[cfg(test)]
use crate::{DeclarationSourceStore, ItemTrees, file_declarations, target_sources};

pub fn fixture_database(workspace: &WorkspaceMetadata) -> rg_source::QueryDatabase {
    let inputs = ajika::Inputs::default()
        .edit(|update| workspace.write_inputs(update))
        .expect("fixture workspace inputs should install");
    // The fixture owns the whole database lifetime and can keep all its results until teardown.
    rg_source::QueryDatabase::memory(
        inputs,
        rg_source::QueryServices::default(),
        ajika::DatabaseOptions::new(ajika::Retention {
            max_entries: usize::MAX,
            max_bytes: u64::MAX,
            max_idle: Duration::MAX,
        }),
    )
}

/// Item trees and their source locations collected from fixture targets.
#[cfg(test)]
pub(crate) struct ItemTreeFixture {
    _fixture: CrateFixture,
    workspace: WorkspaceMetadata,
    interner: ajika::Interner,
    trees: Vec<ItemTrees>,
    locations: Vec<HashMap<FileId, Arc<DeclarationSourceStore<Span>>>>,
}

#[cfg(test)]
impl ItemTreeFixture {
    pub fn build(fixture: &str) -> Self {
        let fixture = fixture_crate(fixture);
        let workspace = if fixture.headers().next().is_some() {
            WorkspaceMetadata::from_fixture(&fixture.path(""), fixture.headers())
        } else {
            // Manifest fixtures exercise Cargo's edition and target lowering.
            WorkspaceMetadata::for_tests(fixture.metadata(), WorkspaceLoweringConfig::default())
                .expect("fixture workspace metadata should build")
        };
        let mut database = fixture_database(&workspace);
        let (trees, locations) = database
            .run(|db| {
                let mut trees = Vec::new();
                let mut locations = Vec::new();
                for slot in 0..workspace.packages().len() {
                    let mut item_trees = ItemTrees::default();
                    let mut file_locations = HashMap::new();
                    for (krate, _) in workspace.targets_for_package(PackageSlot(slot)) {
                        let sources = target_sources(db, krate)?;
                        for &file in &sources.files {
                            let declarations = file_declarations(db, krate.package, file)?;
                            item_trees.extend_files([declarations.items.value(db)?]);
                            file_locations.insert(file, declarations.sources.value(db)?);
                        }
                    }
                    trees.push(item_trees);
                    locations.push(file_locations);
                }
                Ok((trees, locations))
            })
            .expect("fixture sources and declarations should build");
        Self {
            _fixture: fixture,
            workspace,
            interner: database.interner(),
            trees,
            locations,
        }
    }

    pub fn workspace(&self) -> &WorkspaceMetadata {
        &self.workspace
    }

    pub fn file_path(&self, file: FileId) -> PathBuf {
        file.path(&self.interner)
            .expect("fixture source path should resolve")
    }

    pub fn file_id(&self, path: &Path) -> FileId {
        FileId::for_path(&self.interner, path).expect("fixture source identity should resolve")
    }

    pub fn locations(&self, package: usize, file: FileId) -> &DeclarationSourceStore<Span> {
        &self.locations[package][&file]
    }

    pub fn item_trees(&self, package: usize) -> &ItemTrees {
        &self.trees[package]
    }
}
