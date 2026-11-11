use std::collections::HashMap;

use rg_arena::Arena;
use rg_ir_model::{CrateId, CrateRef, PackageSlot};
use rg_item_tree::ItemTrees;
use rg_parse::{CargoTarget, CargoTargetId, Package, ParseDb};
use rg_source::ReadError;
use rg_workspace::{SysrootSources, TargetKind, WorkspaceLoweringConfig, WorkspaceMetadata};
use test_fixture::{CrateFixture, fixture_crate};

use crate::{
    CrateData, CrateDataSource, CrateDefMapReader, DefMap, DefinitionReader, GeneratedItemStore,
    MacroExpansionLimitReport, build::build_fixture_definitions,
};

pub type GeneratedItemStores = HashMap<CrateRef, GeneratedItemStore>;

/// Definitions collected for one package in kernel fixtures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageDefMaps {
    pub(crate) crates: Arena<CrateId, CrateData>,
    macro_expansion_limits: Vec<MacroExpansionLimitReport>,
}

impl PackageDefMaps {
    pub(crate) fn new(
        crates: Vec<CrateData>,
        macro_expansion_limits: Vec<MacroExpansionLimitReport>,
    ) -> Self {
        Self {
            crates: Arena::from_vec(crates),
            macro_expansion_limits,
        }
    }

    /// Returns all semantic crates in crate-id order.
    pub fn crates(&self) -> &[CrateData] {
        self.crates.as_slice()
    }

    /// Returns one semantic crate by its package-local identity.
    pub fn crate_data(&self, crate_id: CrateId) -> Option<&CrateData> {
        self.crates.get(crate_id)
    }

    /// Finds the semantic crate produced from one parsed Cargo target.
    pub fn crate_for_cargo_target(&self, cargo_target: CargoTargetId) -> Option<CrateId> {
        self.crates
            .iter_with_ids()
            .find_map(|(crate_id, data)| (data.cargo_target() == cargo_target).then_some(crate_id))
    }

    pub fn def_map(&self, crate_id: CrateId) -> Option<&DefMap> {
        self.crate_data(crate_id).map(CrateData::def_map)
    }

    /// Returns bounded diagnostics for crates with invocations that reached the macro recursion limit.
    pub fn macro_expansion_limits(&self) -> &[MacroExpansionLimitReport] {
        &self.macro_expansion_limits
    }
}

/// Completed local arenas used by definition and type-system kernel tests.
#[derive(Debug)]
pub struct DefMapDb {
    pub(crate) packages: Vec<PackageDefMaps>,
}

impl DefMapDb {
    pub fn package(&self, package: PackageSlot) -> Option<&PackageDefMaps> {
        self.packages.get(package.0)
    }

    pub fn reader(&self) -> DefinitionReader<'_> {
        CrateDefMapReader(self)
    }

    pub fn macro_expansion_limit_reports(
        &self,
    ) -> impl Iterator<Item = &MacroExpansionLimitReport> {
        self.packages
            .iter()
            .flat_map(|package| package.macro_expansion_limits())
    }
}

impl CrateDataSource for DefMapDb {
    type Error = ReadError;

    fn crate_data(&self, krate: CrateRef) -> Result<Option<&CrateData>, ReadError> {
        Ok(self
            .package(krate.package)
            .and_then(|package| package.crate_data(krate.crate_id)))
    }
}

/// End-to-end fixture for tests that need name resolution data.
pub struct DefMapFixture {
    _fixture: CrateFixture,
    parse: ParseDb,
    trees: HashMap<CrateRef, ItemTrees>,
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

    pub(crate) fn build_from_crate(fixture: CrateFixture, workspace: WorkspaceMetadata) -> Self {
        let (parse, def_map, trees, generated_items) =
            build_fixture_definitions(&workspace).expect("fixture definitions should build");

        Self {
            _fixture: fixture,
            parse,
            trees,
            def_map,
            generated_items,
            workspace,
        }
    }

    pub fn parse_db(&self) -> &ParseDb {
        &self.parse
    }

    pub fn item_trees(&self, krate: CrateRef) -> &ItemTrees {
        &self.trees[&krate]
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

    pub fn def_map(&self, crate_ref: CrateRef) -> Option<&DefMap> {
        self.def_map
            .package(crate_ref.package)?
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
            .package(package_slot)
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
