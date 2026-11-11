use rg_def_map::{
    CrateData, CrateDataSource, CrateDefMapReader, CrateDependencies, DefMap, DefinitionReader,
};
use rg_ir_model::{CrateId, CrateRef, PackageSlot};
use rg_parse::{CargoTarget, ParseDb};
use rg_source::{QueryContext, QueryDatabase};
use rg_workspace::{TargetKind, WorkspaceMetadata, testonly::WorkspaceFixture};

use crate::{CrateItemSource, ItemReader, ItemStore, TargetDeclarationReader, target_declarations};

/// In-memory queries over fixture sources. The providers own declaration construction.
pub struct SemanticIrFixture {
    database: QueryDatabase,
    parse: ParseDb,
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
        let inputs = database.inputs().clone();
        // Source routing comes from the same declarations used by later fixture queries.
        let parse = database
            .run(|db| {
                let metadata = targets
                    .iter()
                    .map(|&target| target_declarations(db, target)?.metadata.value_ref(db))
                    .collect::<Result<Vec<_>, _>>()?;
                ParseDb::from_target_manifests(
                    workspace.metadata(),
                    &inputs,
                    targets
                        .iter()
                        .copied()
                        .zip(metadata.iter().map(|metadata| &metadata.sources)),
                )
            })
            .expect("fixture source routing should build");
        Self {
            database,
            parse,
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
                    parse: &self.parse,
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
    parse: &'db ParseDb,
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

    pub fn parse_db(&self) -> &'db ParseDb {
        self.parse
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

    pub fn crate_items(&self, target: CrateRef) -> Option<&'db ItemStore> {
        self.declarations
            .items(target)
            .expect("fixture items should load")
    }

    pub fn package_slot_by_name(&self, name: &str) -> PackageSlot {
        PackageSlot(
            self.parse
                .packages()
                .iter()
                .position(|package| package.package_name() == name)
                .unwrap_or_else(|| panic!("fixture package {name} should exist")),
        )
    }

    pub fn target(&self, name: &str, kind: TargetKind) -> (PackageSlot, &'db CargoTarget) {
        let slot = self.package_slot_by_name(name);
        let mut targets = self.parse.packages()[slot.0]
            .targets()
            .iter()
            .filter(|target| target.kind == kind);
        let target = targets.next().expect("fixture target should exist");
        assert!(
            targets.next().is_none(),
            "fixture target selection should be unique"
        );
        (slot, target)
    }

    pub fn crate_ref(&self, name: &str, kind: TargetKind) -> CrateRef {
        let (package, target) = self.target(name, kind);
        CrateRef {
            package,
            crate_id: CrateId(target.id.0),
        }
    }
}
