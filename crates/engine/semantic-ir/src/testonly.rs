use rg_def_map::{
    CrateData, CrateDataSource, CrateDefMapReader, CrateDependencies, DefMap, DefinitionReader,
};
use rg_ir_model::{CrateRef, DefMapRef, PackageSlot};
use rg_source::{QueryContext, QueryDatabase};
use rg_workspace::{CargoTarget, TargetKind, WorkspaceMetadata, testonly::WorkspaceFixture};

use crate::{CrateItemSource, ItemReader, SemanticItemView, TargetDeclarationReader};

/// In-memory queries over fixture sources. The providers own declaration construction.
pub struct SemanticIrFixture {
    database: QueryDatabase,
    targets: Vec<CrateRef>,
    workspace: WorkspaceFixture,
}

impl SemanticIrFixture {
    pub fn build(spec: &str) -> Self {
        Self::from_workspace(WorkspaceFixture::build(spec))
    }

    pub fn build_with_sysroot(spec: &str) -> Self {
        Self::from_workspace(WorkspaceFixture::build(spec).with_sysroot())
    }

    pub fn build_with_fake_sysroot(spec: &str) -> Self {
        Self::from_workspace(WorkspaceFixture::build(spec).with_fake_sysroot())
    }

    pub fn from_workspace(workspace: WorkspaceFixture) -> Self {
        let mut database = rg_item_tree::testonly::fixture_database(workspace.metadata());
        let targets = database
            .update_inputs(|update| CrateDependencies::write_inputs(workspace.metadata(), update))
            .expect("fixture target inputs should install");
        Self {
            database,
            targets,
            workspace,
        }
    }

    pub fn inspect<R>(&mut self, observe: impl FnOnce(SemanticIrView<'_>) -> R) -> R {
        // Borrowed results stay inside the operation, as they do for application queries.
        self.database
            .run(|db| {
                let declarations =
                    TargetDeclarationReader::for_project(db, self.targets.iter().copied());
                Ok(observe(SemanticIrView {
                    db,
                    declarations: &declarations,
                    workspace: self.workspace.metadata(),
                    targets: &self.targets,
                }))
            })
            .expect("fixture query operation should succeed")
    }
}

#[derive(Clone, Copy)]
pub struct SemanticIrView<'db> {
    db: &'db QueryContext<'db>,
    declarations: &'db TargetDeclarationReader<'db>,
    workspace: &'db WorkspaceMetadata,
    targets: &'db [CrateRef],
}

impl<'db> SemanticIrView<'db> {
    pub fn db(&self) -> &'db QueryContext<'db> {
        self.db
    }

    pub fn declarations(&self) -> &'db TargetDeclarationReader<'db> {
        self.declarations
    }

    pub fn definitions(&self) -> DefinitionReader<'db> {
        CrateDefMapReader(self.declarations)
    }

    pub fn items(&self) -> ItemReader<'db> {
        ItemReader(self.declarations)
    }

    pub fn workspace(&self) -> &'db WorkspaceMetadata {
        self.workspace
    }

    pub fn targets(&self) -> &'db [CrateRef] {
        self.targets
    }

    pub fn def_map(&self, target: CrateRef) -> Option<&'db DefMap> {
        self.declarations
            .crate_data(target)
            .expect("fixture definitions should load")
            .map(CrateData::def_map)
    }

    pub fn all_items(&self, origin: DefMapRef) -> Vec<SemanticItemView<'db>> {
        self.declarations
            .all_items(origin)
            .expect("fixture items should load")
    }

    pub fn package_slot_by_name(&self, name: &str) -> PackageSlot {
        PackageSlot(
            self.workspace
                .packages()
                .iter()
                .position(|package| package.name == name)
                .unwrap_or_else(|| panic!("fixture package {name} should exist")),
        )
    }

    pub fn target(&self, name: &str, kind: TargetKind) -> (CrateRef, &'db CargoTarget) {
        let slot = self.package_slot_by_name(name);
        let mut targets = self
            .workspace
            .targets_for_package(slot)
            .filter(|(_, target)| target.kind == kind);
        let target = targets.next().expect("fixture target should exist");
        assert!(
            targets.next().is_none(),
            "fixture target selection should be unique"
        );
        target
    }

    pub fn crate_ref(&self, name: &str, kind: TargetKind) -> CrateRef {
        self.target(name, kind).0
    }
}
