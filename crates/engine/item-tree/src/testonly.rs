use std::time::Duration;

use rg_parse::ParseDb;
use rg_workspace::{WorkspaceLoweringConfig, WorkspaceMetadata};
use test_fixture::{CrateFixture, fixture_crate};

use crate::ItemTreeDb;

pub fn fixture_database(workspace: &WorkspaceMetadata) -> ajika::Database {
    let inputs = ajika::Inputs::default()
        .edit(|update| workspace.write_inputs(update))
        .expect("fixture workspace inputs should install");
    // The fixture owns the whole database lifetime and can keep all its results until teardown.
    ajika::Database::memory(
        inputs,
        (),
        ajika::Retention {
            max_entries: usize::MAX,
            max_bytes: u64::MAX,
            max_idle: Duration::MAX,
        },
    )
}

/// End-to-end fixture for tests that need parsed files and item trees.
pub struct ItemTreeFixture {
    _fixture: CrateFixture,
    parse: ParseDb,
    item_tree: ItemTreeDb,
}

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

    pub fn build_from_crate(fixture: CrateFixture, workspace: &WorkspaceMetadata) -> Self {
        let mut database = fixture_database(workspace);
        let mut parse =
            ParseDb::build(workspace, database.interner()).expect("fixture parse db should build");
        let packages = (0..parse.package_count()).collect::<Vec<_>>();
        let item_tree = ItemTreeDb::build_packages(&mut database, &mut parse, &packages)
            .expect("fixture item tree db should build");

        Self {
            _fixture: fixture,
            parse,
            item_tree,
        }
    }

    pub fn parse_db(&self) -> &ParseDb {
        &self.parse
    }

    pub fn item_tree_db(&self) -> &ItemTreeDb {
        &self.item_tree
    }

    pub fn build_inputs_mut(&mut self) -> (&mut ParseDb, &mut ItemTreeDb) {
        (&mut self.parse, &mut self.item_tree)
    }
}
