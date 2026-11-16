use std::path::PathBuf;

use rg_arena::Arena;
use rg_ir_model::{CrateId, CrateRef, FileId, PackageSlot};
use rg_source::ReadError;
use rg_workspace::{
    CargoTarget, TargetKind, WorkspaceLoweringConfig, WorkspaceMetadata, testonly::WorkspaceFixture,
};
use test_fixture::{CrateFixture, fixture_crate};

use crate::{
    CrateData, CrateDataSource, CrateDefMapReader, DefMap, DefinitionReader,
    MacroExpansionLimitReport, build::build_fixture_definitions,
};

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

    /// Returns one semantic crate by its package-local identity.
    pub fn crate_data(&self, crate_id: CrateId) -> Option<&CrateData> {
        self.crates.get(crate_id)
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
    fn crate_data(&self, krate: CrateRef) -> Result<Option<&CrateData>, ReadError> {
        Ok(self
            .package(krate.package)
            .and_then(|package| package.crate_data(krate.crate_id)))
    }
}

/// End-to-end fixture for tests that need name resolution data.
pub struct DefMapFixture {
    workspace: WorkspaceFixture,
    interner: ajika::Interner,
    def_map: DefMapDb,
}

impl DefMapFixture {
    pub fn build(fixture: &str) -> Self {
        Self::build_workspace(WorkspaceFixture::build(fixture))
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
        Self::build_workspace(WorkspaceFixture::build(fixture).with_sysroot())
    }

    pub fn build_with_fake_sysroot(fixture: &str) -> Self {
        Self::build_workspace(WorkspaceFixture::build(fixture).with_fake_sysroot())
    }

    pub(crate) fn build_from_crate(fixture: CrateFixture, workspace: WorkspaceMetadata) -> Self {
        Self::build_workspace(WorkspaceFixture::from_metadata(fixture, workspace))
    }

    fn build_workspace(workspace: WorkspaceFixture) -> Self {
        let (interner, def_map) = build_fixture_definitions(workspace.metadata())
            .expect("fixture definitions should build");
        Self {
            workspace,
            interner,
            def_map,
        }
    }

    pub fn workspace(&self) -> &WorkspaceMetadata {
        self.workspace.metadata()
    }

    pub fn file_path(&self, file: FileId) -> PathBuf {
        file.path(&self.interner)
            .expect("fixture file path should resolve")
    }

    pub fn def_map_db(&self) -> &DefMapDb {
        &self.def_map
    }

    pub fn def_map(&self, crate_ref: CrateRef) -> Option<&DefMap> {
        self.def_map
            .package(crate_ref.package)?
            .def_map(crate_ref.crate_id)
    }

    pub fn package_slot_by_name(&self, package_name: &str) -> PackageSlot {
        self.workspace()
            .packages()
            .iter()
            .enumerate()
            .find_map(|(idx, package)| (package.name == package_name).then_some(PackageSlot(idx)))
            .unwrap_or_else(|| panic!("fixture package `{package_name}` should exist"))
    }

    pub fn crate_ref(&self, package_name: &str, expected_kind: TargetKind) -> CrateRef {
        self.target(package_name, expected_kind).0
    }

    pub fn target(
        &self,
        package_name: &str,
        expected_kind: TargetKind,
    ) -> (CrateRef, &CargoTarget) {
        let slot = self.package_slot_by_name(package_name);
        self.workspace()
            .targets_for_package(slot)
            .find(|(_, target)| target.kind == expected_kind)
            .expect("fixture target should exist")
    }
}
