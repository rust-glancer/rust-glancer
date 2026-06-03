use rg_ir_model::TargetRef;
use rg_ir_storage::DefMap;
use rg_item_tree::{ItemTreeDb, testonly::ItemTreeFixture};
use rg_parse::{Package, ParseDb, Target};
use rg_workspace::{SysrootSources, TargetKind, WorkspaceMetadata};
use test_fixture::{CrateFixture, fixture_crate};

use crate::{DefMapDb, DefMapFinalizationStats, PackageSlot};

/// End-to-end fixture for tests that need name resolution data.
pub struct DefMapFixture {
    item_tree: ItemTreeFixture,
    def_map: DefMapDb,
}

impl DefMapFixture {
    pub fn build(fixture: &str) -> Self {
        let fixture = fixture_crate(fixture);
        let workspace = WorkspaceMetadata::from_cargo(fixture.metadata())
            .expect("fixture workspace metadata should build");
        Self::build_from_crate(fixture, workspace)
    }

    pub fn build_with_sysroot(fixture: &str) -> Self {
        let fixture = fixture_crate(fixture);
        let sysroot = SysrootSources::from_library_root(fixture.path("sysroot/library"))
            .expect("fixture sysroot should be complete");
        let workspace = WorkspaceMetadata::from_cargo(fixture.metadata())
            .expect("fixture workspace metadata should build")
            .with_sysroot_sources(Some(sysroot));
        Self::build_from_crate(fixture, workspace)
    }

    pub fn build_with_finalization_stats(fixture: &str) -> (Self, DefMapFinalizationStats) {
        let fixture = fixture_crate(fixture);
        let workspace = WorkspaceMetadata::from_cargo(fixture.metadata())
            .expect("fixture workspace metadata should build");
        let mut stats = DefMapFinalizationStats::default();
        let db =
            Self::build_from_crate_with_finalization_stats(fixture, workspace, Some(&mut stats));
        (db, stats)
    }

    pub fn build_from_crate(fixture: CrateFixture, workspace: WorkspaceMetadata) -> Self {
        Self::build_from_crate_with_finalization_stats(fixture, workspace, None)
    }

    fn build_from_crate_with_finalization_stats(
        fixture: CrateFixture,
        workspace: WorkspaceMetadata,
        finalization_stats: Option<&mut DefMapFinalizationStats>,
    ) -> Self {
        let item_tree = ItemTreeFixture::build_from_crate(fixture, &workspace);
        let mut builder =
            DefMapDb::builder(&workspace, item_tree.parse_db(), item_tree.item_tree_db());
        if let Some(stats) = finalization_stats {
            builder = builder.finalization_stats(stats);
        }
        let def_map = builder.build().expect("fixture def map db should build");

        Self { item_tree, def_map }
    }

    pub fn parse_db(&self) -> &ParseDb {
        self.item_tree.parse_db()
    }

    pub fn item_tree_db(&self) -> &ItemTreeDb {
        self.item_tree.item_tree_db()
    }

    pub fn def_map_db(&self) -> &DefMapDb {
        &self.def_map
    }

    pub fn resident_def_map(&self, target: TargetRef) -> Option<&DefMap> {
        self.def_map
            .resident_package(target.package)?
            .def_map(target.target)
    }

    pub fn package_slot_by_name(&self, package_name: &str) -> PackageSlot {
        self.parse_db()
            .packages()
            .iter()
            .enumerate()
            .find_map(|(idx, package)| {
                (package.package_name() == package_name).then_some(PackageSlot(idx))
            })
            .unwrap_or_else(|| panic!("fixture package `{package_name}` should exist"))
    }

    pub fn target_ref(&self, package_name: &str, expected_kind: TargetKind) -> TargetRef {
        let (package_slot, target) = self.target(package_name, expected_kind);
        TargetRef {
            package: package_slot,
            target: target.id,
        }
    }

    pub fn target(&self, package_name: &str, expected_kind: TargetKind) -> (PackageSlot, &Target) {
        let package_slot = self.package_slot_by_name(package_name);
        let package = self
            .parse_db()
            .package(package_slot.0)
            .expect("fixture package slot should exist");
        let target = package
            .targets()
            .iter()
            .find(|target| target.kind == expected_kind)
            .unwrap_or_else(|| {
                panic!(
                    "fixture package `{package_name}` should have a {:?} target",
                    expected_kind
                )
            });

        (package_slot, target)
    }

    pub fn package(&self, package: PackageSlot) -> Option<&Package> {
        self.parse_db().package(package.0)
    }
}
