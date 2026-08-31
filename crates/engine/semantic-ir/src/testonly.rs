use crate::ItemStore;
use rg_def_map::testonly::DefMapFixture;
use rg_def_map::{DefMap, PackageSlot};
use rg_ir_model::CrateRef;
use rg_package_store::{PackageStore, PackageSubset};
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

    pub fn build_with_fake_sysroot(fixture: &str) -> Self {
        Self::build_from_def_map(DefMapFixture::build_with_fake_sysroot(fixture))
    }

    pub fn build_from_def_map(mut def_map: DefMapFixture) -> Self {
        let package_count = def_map.parse_db().package_count();
        let packages = (0..package_count).map(PackageSlot).collect::<Vec<_>>();
        let subset = PackageSubset::all(package_count);
        let baseline = SemanticIrDb::from_package_store(PackageStore::all_offloaded(package_count));
        let generated_items = def_map.take_generated_items();

        // Fixtures build every package from source, but still enter Semantic IR through the same
        // baseline-replacement path used by project construction. The generated declaration store
        // is dropped immediately afterward so every downstream fixture query exercises the same
        // retained-state boundary as a real project.
        let semantic_ir = baseline
            .build_packages(
                def_map.item_tree_db(),
                def_map.def_map_db(),
                &generated_items,
                &packages,
                rg_def_map::DefMapLoader::resident_only("fixture DefMap"),
                crate::SemanticIrLoader::resident_only("fixture Semantic IR"),
                &subset,
            )
            .expect("fixture semantic ir db should build");
        drop(generated_items);

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

    pub fn resident_def_map(&self, crate_ref: CrateRef) -> Option<&DefMap> {
        self.def_map.resident_def_map(crate_ref)
    }

    pub fn resident_crate_ir(&self, crate_ref: CrateRef) -> Option<&ItemStore> {
        self.semantic_ir
            .resident_package(crate_ref.package)?
            .crate_items(crate_ref.crate_id)
    }
}
