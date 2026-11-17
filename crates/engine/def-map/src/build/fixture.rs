use std::collections::HashMap;

use rg_ir_model::{CrateRef, PackageSlot};
use rg_item_tree::testonly::fixture_database;
use rg_source::ReadError;
use rg_workspace::WorkspaceMetadata;

use super::target::CrateDependencies;
use crate::{
    CrateData, CrateDefMapReader, CrateDefinitionSource, CrateMetadata, DefMapQuery, DefMapView,
    MacroExpansionLimitReport,
    testonly::{DefMapDb, PackageDefMaps},
};

pub(crate) fn build_fixture_definitions(
    workspace: &WorkspaceMetadata,
) -> anyhow::Result<(ajika::Interner, DefMapDb)> {
    let mut database = fixture_database(workspace);
    let targets =
        database.update_inputs(|update| CrateDependencies::write_inputs(workspace, update))?;

    let (mut completed, reports) = database.run(|db| {
        let mut completed = FixtureDefinitions::default();
        let mut reports = HashMap::<PackageSlot, Vec<MacroExpansionLimitReport>>::new();
        // Definition tests inspect local arenas while exercising the production target builder.
        for krate in targets {
            let dependencies = DefMapQuery::new(CrateDefMapReader(&completed));
            let (built, _) = CrateData::build_target(db, krate, &dependencies)?;
            completed.0.insert(krate, built.data);
            reports
                .entry(krate.package)
                .or_default()
                .extend(built.macro_expansion_limit);
        }
        Ok((completed, reports))
    })?;

    let mut reports = reports;
    let packages = workspace
        .packages()
        .iter()
        .enumerate()
        .map(|(slot, _)| {
            let slot = PackageSlot(slot);
            let crates = workspace
                .targets_for_package(slot)
                .map(|(krate, _)| {
                    completed
                        .0
                        .remove(&krate)
                        .expect("target definitions were completed")
                })
                .collect();
            PackageDefMaps::new(crates, reports.remove(&slot).unwrap_or_default())
        })
        .collect();
    Ok((database.interner(), DefMapDb { packages }))
}

#[derive(Default)]
struct FixtureDefinitions(HashMap<CrateRef, CrateData>);

impl CrateDefinitionSource for FixtureDefinitions {
    fn crate_metadata(&self, krate: CrateRef) -> Result<Option<&CrateMetadata>, ReadError> {
        Ok(self.0.get(&krate).map(CrateData::metadata))
    }

    fn definitions(&self, krate: CrateRef) -> Result<Option<DefMapView<'_>>, ReadError> {
        Ok(self.0.get(&krate).map(|data| data.def_map().view()))
    }
}
