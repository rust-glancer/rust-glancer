use rg_parse::ParseDb;
use rg_workspace::{WorkspaceLoweringConfig, WorkspaceMetadata};
use test_fixture::{CrateFixture, fixture_crate};

use crate::{ItemTreeDb, PackageNameInterners};

/// End-to-end fixture for tests that need parsed files and item trees.
pub struct ItemTreeFixture {
    _fixture: CrateFixture,
    parse: ParseDb,
    item_tree: ItemTreeDb,
}

impl ItemTreeFixture {
    pub fn build(fixture: &str) -> Self {
        let fixture = fixture_crate(fixture);
        let workspace =
            WorkspaceMetadata::for_tests(fixture.metadata(), WorkspaceLoweringConfig::default())
                .expect("fixture workspace metadata should build");
        Self::build_from_crate(fixture, &workspace)
    }

    pub fn build_from_crate(fixture: CrateFixture, workspace: &WorkspaceMetadata) -> Self {
        let mut parse = ParseDb::build(workspace).expect("fixture parse db should build");
        let mut names = PackageNameInterners::new(parse.package_count());
        let item_tree =
            ItemTreeDb::build(&mut parse, &mut names).expect("fixture item tree db should build");

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
}
