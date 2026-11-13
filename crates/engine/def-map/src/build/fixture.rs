use std::collections::HashMap;

use rg_ir_model::{CrateId, CrateRef, PackageSlot};
use rg_item_tree::testonly::fixture_database;
use rg_parse::ParseDb;
use rg_source::ReadError;
use rg_workspace::WorkspaceMetadata;

use super::target::CrateDependencies;
use crate::{
    CrateData, CrateDataSource, CrateDefMapReader, DefMapQuery, MacroExpansionLimitReport,
    testonly::{DefMapDb, PackageDefMaps},
};

pub(crate) fn build_fixture_definitions(
    workspace: &WorkspaceMetadata,
) -> anyhow::Result<(ParseDb, DefMapDb)> {
    let mut database = fixture_database(workspace);
    let targets =
        database.update_inputs(|update| CrateDependencies::write_inputs(workspace, update))?;

    let inputs = database.inputs().clone();
    let (parse, mut completed, reports) = database.run(|db| {
        let mut completed = FixtureDefinitions::default();
        let mut manifests = Vec::new();
        let mut reports = HashMap::<PackageSlot, Vec<MacroExpansionLimitReport>>::new();
        // Definition tests inspect local arenas while exercising the production target builder.
        for krate in targets {
            let dependencies = DefMapQuery::new(CrateDefMapReader(&completed));
            let (built, _, manifest) = CrateData::build_target(db, krate, &dependencies)?;
            completed.0.insert(krate, built.data);
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
        Ok((parse, completed, reports))
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
    Ok((parse, DefMapDb { packages }))
}

#[derive(Default)]
struct FixtureDefinitions(HashMap<CrateRef, CrateData>);

impl CrateDataSource for FixtureDefinitions {
    fn crate_data(&self, krate: CrateRef) -> Result<Option<&CrateData>, ReadError> {
        Ok(self.0.get(&krate))
    }
}
