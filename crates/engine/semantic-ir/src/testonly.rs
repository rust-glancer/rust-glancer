use rg_def_map::testonly::DefMapFixture;
use rg_ir_model::TargetRef;
use rg_ir_storage::{DefMap, ItemStore};
use rg_parse::ParseDb;

use crate::SemanticIrDb;

/// End-to-end fixture for tests that need semantic item data.
pub struct SemanticIrFixture {
    def_map: DefMapFixture,
    semantic_ir: SemanticIrDb,
}

impl SemanticIrFixture {
    pub fn build(fixture: &str) -> Self {
        Self::build_from_def_map(DefMapFixture::build(fixture))
    }

    pub fn build_with_sysroot(fixture: &str) -> Self {
        Self::build_from_def_map(DefMapFixture::build_with_sysroot(fixture))
    }

    pub fn build_from_def_map(def_map: DefMapFixture) -> Self {
        let semantic_ir = SemanticIrDb::builder(def_map.item_tree_db(), def_map.def_map_db())
            .build()
            .expect("fixture semantic ir db should build");

        Self {
            def_map,
            semantic_ir,
        }
    }

    pub fn parse_db(&self) -> &ParseDb {
        self.def_map.parse_db()
    }

    pub fn def_map_fixture(&self) -> &DefMapFixture {
        &self.def_map
    }

    pub fn def_map_db(&self) -> &rg_def_map::DefMapDb {
        self.def_map.def_map_db()
    }

    pub fn semantic_ir_db(&self) -> &SemanticIrDb {
        &self.semantic_ir
    }

    pub fn resident_def_map(&self, target: TargetRef) -> Option<&DefMap> {
        self.def_map.resident_def_map(target)
    }

    pub fn resident_target_ir(&self, target: TargetRef) -> Option<&ItemStore> {
        self.semantic_ir
            .resident_package(target.package)?
            .target(target.target)
    }
}
