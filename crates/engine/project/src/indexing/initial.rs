//! Demand the roots of a saved project and restore its routing from their manifests.

use std::collections::HashSet;

use rg_ir_model::CrateRef;
use rg_parse::ParseDb;
use rg_profile::{ProfileMeasurement, ProfileThreadContext};
use rg_semantic_ir::target_declarations;
use rg_source::{QueryDatabase, SourceInput};
use rg_workspace::WorkspaceMetadata;

use crate::{
    MacroExpansionLimitBuildSummary,
    profile::metric,
    state::{ProjectGenerationId, ProjectState},
};

pub(crate) fn build_state(
    database: &mut QueryDatabase,
    workspace: WorkspaceMetadata,
    targets: Vec<CrateRef>,
) -> ajika::Result<ProjectState> {
    let inputs = database.inputs().clone();
    let operation = metric::QUERY_OPERATION.start_timer();
    let (parse, module_files, reports) = database.run(|db| {
        // Targets appear in dependency-first order. Concurrent demands share Ajika's completed
        // results while a target's import fixed point keeps its unfinished scopes local.
        let timer = metric::DECLARATIONS.start_timer();
        let profile = ProfileThreadContext::capture();
        let declarations = db.parallel_map(&targets, |db, &target| {
            let _profile = profile.enter();
            let declarations = target_declarations(db, target)?;
            let metadata = declarations.metadata.value(db)?;
            Ok((declarations, metadata))
        })?;
        timer.finish();
        let parse = ParseDb::from_target_manifests(
            &workspace,
            &inputs,
            targets
                .iter()
                .copied()
                .zip(declarations.iter().map(|(_, metadata)| &metadata.sources)),
        )?;

        // Warm replay can capture inputs while checking old dependencies. Validate the resulting
        // view inside the operation, before Ajika can checkpoint it.
        let timer = metric::SOURCE_VALIDATION.start_timer();
        SourceInput::validate_saved(&inputs, &db.interner())?;
        timer.finish();
        let module_files = targets
            .iter()
            .copied()
            .zip(
                declarations
                    .iter()
                    .map(|(_, metadata)| metadata.module_files.clone()),
            )
            .collect();
        let reports = declarations
            .iter()
            .filter_map(|(_, metadata)| metadata.macro_expansion_limit.clone())
            .collect::<Vec<_>>();
        Ok((parse, module_files, reports))
    })?;
    operation.finish();

    // Take the operation's counters after checkpointing so writes appear in the same report.
    let stats = database.statistics();
    metric::EXECUTED.add(stats.executed);
    metric::VALIDATED.add(stats.validated);
    metric::HITS.add(stats.hits);
    metric::DECODED.add(stats.decoded);
    metric::METADATA_READS.add(stats.metadata_reads);
    metric::PAYLOAD_BYTES_READ.record(ProfileMeasurement::Bytes(stats.bytes_read));
    metric::PAYLOAD_BYTES_WRITTEN.record(ProfileMeasurement::Bytes(stats.bytes_written));
    metric::METADATA_BYTES_WRITTEN.record(ProfileMeasurement::Bytes(stats.metadata_bytes_written));

    let retained: HashSet<_> = parse
        .packages()
        .iter()
        .flat_map(|package| package.parsed_files().map(|file| file.file_id()))
        .collect();
    SourceInput::retire_detached_sources(database, |file| retained.contains(&file))?;
    SourceInput::evict_saved_text(database.inputs());
    Ok(ProjectState {
        generation_id: ProjectGenerationId::fresh(),
        workspace,
        parse,
        targets,
        module_files,
        macro_expansion_limit_summary: MacroExpansionLimitBuildSummary::capture(&reports),
        reports,
    })
}
