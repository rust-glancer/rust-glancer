use rg_ir_model::{CrateRef, PackageSlot};
use rg_item_tree::{ItemTreeDb, testonly::ItemTreeFixture};
use rg_parse::{CargoTarget, Package, ParseDb};
use rg_text::PackageNameInterners;
use rg_workspace::{SysrootSources, TargetKind, WorkspaceLoweringConfig, WorkspaceMetadata};
use test_fixture::{CrateFixture, fixture_crate};

use crate::{
    DefMap, DefMapBuildOutput, DefMapDb, GeneratedItemStores, MacroExpansionPerformancePreference,
    MacroSourceFileLoader, MacroSourceFileRequest, MacroSourceFileResolution,
};

/// End-to-end fixture for tests that need name resolution data.
pub struct DefMapFixture {
    item_tree: ItemTreeFixture,
    def_map: DefMapDb,
    generated_items: GeneratedItemStores,
    workspace: WorkspaceMetadata,
}

impl DefMapFixture {
    pub fn build(fixture: &str) -> Self {
        let fixture = fixture_crate(fixture);
        let workspace = Self::workspace_metadata(&fixture);
        Self::build_from_crate(fixture, workspace)
    }

    /// Loads Cargo metadata to exercise workspace lowering options. For declaration fixtures,
    /// put cfg facts in file headers and use `build`.
    pub fn build_with_workspace_config(fixture: &str, config: WorkspaceLoweringConfig) -> Self {
        let fixture = fixture_crate(fixture);
        let workspace = WorkspaceMetadata::for_tests(fixture.metadata(), config)
            .expect("fixture workspace metadata should build");
        Self::build_from_crate(fixture, workspace)
    }

    pub fn build_with_sysroot(fixture: &str) -> Self {
        let fixture = fixture_crate(fixture);
        let sysroot = SysrootSources::from_library_root(fixture.path("sysroot/library"))
            .expect("fixture sysroot should be complete");
        let workspace = Self::workspace_metadata(&fixture).with_sysroot_sources(Some(sysroot));
        Self::build_from_crate(fixture, workspace)
    }

    pub fn build_with_fake_sysroot(fixture: &str) -> Self {
        let fixture = fixture_crate(fixture).with_fake_sysroot();
        let sysroot = SysrootSources::from_library_root(fixture.path("sysroot/library"))
            .expect("fake sysroot should be complete");
        let workspace = Self::workspace_metadata(&fixture).with_sysroot_sources(Some(sysroot));
        Self::build_from_crate(fixture, workspace)
    }

    fn workspace_metadata(fixture: &CrateFixture) -> WorkspaceMetadata {
        // Semantic fixtures declare the graph they need. A manifest-based fixture still runs
        // Cargo so tests of discovery and lowering can exercise that boundary explicitly.
        if fixture.headers().next().is_some() {
            WorkspaceMetadata::from_fixture(&fixture.path(""), fixture.headers())
        } else {
            WorkspaceMetadata::for_tests(fixture.metadata(), WorkspaceLoweringConfig::default())
                .expect("fixture workspace metadata should build")
        }
    }

    pub fn build_from_crate(fixture: CrateFixture, workspace: WorkspaceMetadata) -> Self {
        let mut item_tree = ItemTreeFixture::build_from_crate(fixture, &workspace);
        let mut names = PackageNameInterners::new(item_tree.parse_db().package_count());
        let (parse, trees) = item_tree.build_inputs_mut();
        let output = build_source_closed_def_map(&workspace, parse, trees, &mut names);
        let (def_map, generated_items) = output.into_parts();

        Self {
            item_tree,
            def_map,
            generated_items,
            workspace,
        }
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

    pub fn take_generated_items(&mut self) -> GeneratedItemStores {
        std::mem::take(&mut self.generated_items)
    }

    pub fn workspace(&self) -> &WorkspaceMetadata {
        &self.workspace
    }

    pub fn resident_def_map(&self, crate_ref: CrateRef) -> Option<&DefMap> {
        self.def_map
            .resident_package(crate_ref.package)?
            .def_map(crate_ref.crate_id)
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

    pub fn crate_ref(&self, package_name: &str, expected_kind: TargetKind) -> CrateRef {
        let (package_slot, target) = self.target(package_name, expected_kind);
        let crate_id = self
            .def_map
            .resident_package(package_slot)
            .and_then(|package| package.crate_for_cargo_target(target.id))
            .expect("fixture target should have a semantic crate");
        CrateRef {
            package: package_slot,
            crate_id,
        }
    }

    pub fn target(
        &self,
        package_name: &str,
        expected_kind: TargetKind,
    ) -> (PackageSlot, &CargoTarget) {
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

/// Lower-layer fixtures keep their source set closed after ItemTree lowering.
/// Generated file-discovery behavior belongs in project fixtures with a real source loader.
pub(crate) struct SourceClosedMacroSourceFileLoader;

impl MacroSourceFileLoader for SourceClosedMacroSourceFileLoader {
    fn load_files(
        &mut self,
        _parse: &mut ParseDb,
        _item_tree: &mut ItemTreeDb,
        _names: &mut PackageNameInterners,
        requests: &[MacroSourceFileRequest],
    ) -> anyhow::Result<Vec<MacroSourceFileResolution>> {
        anyhow::bail!(
            "fixture requested {} macro source file(s); use an rg_project fixture for source-discovery behavior",
            requests.len(),
        );
    }
}

pub(crate) fn build_source_closed_def_map(
    workspace: &WorkspaceMetadata,
    parse: &mut ParseDb,
    item_tree: &mut ItemTreeDb,
    names: &mut PackageNameInterners,
) -> DefMapBuildOutput {
    let package_count = parse.package_count();
    let baseline = DefMapDb::all_offloaded(package_count);
    let baseline_read = baseline.read_txn(crate::DefMapLoader::resident_only(
        "source-closed DefMap fixture",
    ));
    let packages = (0..package_count).map(PackageSlot).collect::<Vec<_>>();
    baseline
        .build_packages(
            &baseline_read,
            workspace,
            parse,
            item_tree,
            &packages,
            true,
            names,
            MacroExpansionPerformancePreference::default(),
            &mut SourceClosedMacroSourceFileLoader,
        )
        .expect("fixture DefMap should build")
}
