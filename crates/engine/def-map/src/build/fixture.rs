use std::collections::HashMap;

use rg_ir_model::{CrateId, CrateRef, PackageSlot};
use rg_item_tree::{ItemTrees, testonly::fixture_database};
use rg_parse::ParseDb;
use rg_source::ReadError;
use rg_workspace::WorkspaceMetadata;

use super::target::CrateDependencies;
use crate::{
    CrateData, CrateDataSource, CrateDefMapReader, DefMapQuery, MacroExpansionLimitReport,
    testonly::{DefMapDb, GeneratedItemStores, PackageDefMaps},
};

pub(crate) fn build_fixture_definitions(
    workspace: &WorkspaceMetadata,
) -> anyhow::Result<(
    ParseDb,
    DefMapDb,
    HashMap<CrateRef, ItemTrees>,
    GeneratedItemStores,
)> {
    let mut database = fixture_database(workspace);
    let targets =
        database.update_inputs(|update| CrateDependencies::write_inputs(workspace, update))?;

    let inputs = database.inputs().clone();
    let (parse, mut completed, trees, generated, reports) = database.run(|db| {
        let mut completed = FixtureDefinitions::default();
        let mut trees = HashMap::new();
        let mut generated = GeneratedItemStores::new();
        let mut manifests = Vec::new();
        let mut reports = HashMap::<PackageSlot, Vec<MacroExpansionLimitReport>>::new();
        // The production target builder can read dependencies from local arenas here, so
        // downstream kernel fixtures can borrow their definitions without a runtime operation.
        for krate in targets {
            let dependencies = DefMapQuery::new(CrateDefMapReader(&completed));
            let (built, item_trees, manifest) = CrateData::build_target(db, krate, &dependencies)?;
            completed.0.insert(krate, built.data);
            trees.insert(krate, item_trees);
            generated.insert(krate, built.generated_items);
            manifests.push((krate, manifest));
            reports
                .entry(krate.package)
                .or_default()
                .extend(built.macro_expansion_limit);
        }
        let parse = ParseDb::from_target_manifests(
            workspace,
            &inputs,
            manifests.iter().map(|(krate, manifest)| (*krate, manifest)),
        )?;
        Ok((parse, completed, trees, generated, reports))
    })?;

    let mut reports = reports;
    let packages = parse
        .packages()
        .iter()
        .enumerate()
        .map(|(slot, package)| {
            let slot = PackageSlot(slot);
            let crates = package
                .targets()
                .iter()
                .map(|target| {
                    completed
                        .0
                        .remove(&CrateRef {
                            package: slot,
                            crate_id: CrateId(target.id.0),
                        })
                        .expect("target definitions were completed")
                })
                .collect();
            PackageDefMaps::new(crates, reports.remove(&slot).unwrap_or_default())
        })
        .collect();
    Ok((parse, DefMapDb { packages }, trees, generated))
}

#[derive(Default)]
struct FixtureDefinitions(HashMap<CrateRef, CrateData>);

impl CrateDataSource for FixtureDefinitions {
    type Error = ReadError;

    fn crate_data(&self, krate: CrateRef) -> Result<Option<&CrateData>, ReadError> {
        Ok(self.0.get(&krate))
    }
}
