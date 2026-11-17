use std::time::Duration;

#[cfg(test)]
use rg_ir_model::{CrateId, CrateRef, PackageSlot};
#[cfg(test)]
use rg_parse::{ParseDb, TargetFileManifest};
#[cfg(test)]
use rg_workspace::WorkspaceLoweringConfig;
use rg_workspace::WorkspaceMetadata;
#[cfg(test)]
use test_fixture::{CrateFixture, fixture_crate};

#[cfg(test)]
use crate::{ItemTrees, file_declarations, target_sources};

pub fn fixture_database(workspace: &WorkspaceMetadata) -> rg_source::QueryDatabase {
    let inputs = ajika::Inputs::default()
        .edit(|update| workspace.write_inputs(update))
        .expect("fixture workspace inputs should install");
    // The fixture owns the whole database lifetime and can keep all its results until teardown.
    rg_source::QueryDatabase::memory(
        inputs,
        rg_source::QueryServices::default(),
        ajika::Retention {
            max_entries: usize::MAX,
            max_bytes: u64::MAX,
            max_idle: Duration::MAX,
        },
    )
}

/// End-to-end fixture for tests that need parsed files and item trees.
#[cfg(test)]
pub(crate) struct ItemTreeFixture {
    _fixture: CrateFixture,
    parse: ParseDb,
    trees: Vec<ItemTrees>,
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
        Self::build_from_crate(fixture, &workspace)
    }

    fn build_from_crate(fixture: CrateFixture, workspace: &WorkspaceMetadata) -> Self {
        let mut database = fixture_database(workspace);
        let inputs = database.inputs().clone();
        let (parse, trees) = database
            .run(|db| {
                let mut manifests = Vec::new();
                let mut trees = Vec::new();
                for (slot, package) in workspace.packages().iter().enumerate() {
                    let mut item_trees = ItemTrees::default();
                    for (id, _) in package.analyzed_targets().enumerate() {
                        let krate = CrateRef {
                            package: PackageSlot(slot),
                            crate_id: CrateId(id),
                        };
                        let sources = target_sources(db, krate)?;
                        for &file in &sources.files {
                            item_trees.extend_files([file_declarations(db, krate.package, file)?]);
                        }
                        manifests.push((
                            krate,
                            TargetFileManifest {
                                root: sources.root,
                                files: sources.files.clone(),
                            },
                        ));
                    }
                    trees.push(item_trees);
                }
                let parse = ParseDb::from_target_manifests(
                    workspace,
                    &inputs,
                    manifests.iter().map(|(krate, manifest)| (*krate, manifest)),
                )?;
                Ok((parse, trees))
            })
            .expect("fixture sources and declarations should build");
        Self {
            _fixture: fixture,
            parse,
            trees,
        }
    }

    pub fn parse_db(&self) -> &ParseDb {
        &self.parse
    }

    pub fn item_trees(&self, package: usize) -> &ItemTrees {
        &self.trees[package]
    }
}
