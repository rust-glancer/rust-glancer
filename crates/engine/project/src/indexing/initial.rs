//! Demand saved declarations and retain their source-membership facts for routing.

use std::time::Instant;

use rg_ir_model::CrateRef;
use rg_profile::{ProfileMeasurement, ProfileThreadContext};
use rg_semantic_ir::{ItemLookupEnv, target_declarations};
use rg_source::{QueryDatabase, SourceInput};
use rg_workspace::WorkspaceMetadata;

use super::progress::{IndexingProgress, LOG_TARGET};
use crate::{
    MacroExpansionLimitBuildSummary,
    profile::metric,
    source_index::ProjectSourceIndex,
    state::{ProjectGenerationId, ProjectState},
};

#[tracing::instrument(
    target = "rg_project::indexing",
    level = "debug",
    skip_all,
    fields(rg.workspace = %workspace.workspace_root().display())
)]
pub(crate) fn build_state(
    database: &mut QueryDatabase,
    workspace: WorkspaceMetadata,
    targets: Vec<CrateRef>,
) -> ajika::Result<ProjectState> {
    let inputs = database.inputs().clone();
    let operation = metric::QUERY_OPERATION.start_timer();
    let mut finalization_started = None;
    let (sources, reports) = database.run(|db| {
        // Targets appear in dependency-first order. Concurrent demands share Ajika's completed
        // results while a target's import fixed point keeps its unfinished scopes local.
        let timer = metric::DECLARATIONS.start_timer();
        let profile = ProfileThreadContext::capture();
        let progress = tracing::enabled!(target: LOG_TARGET, tracing::Level::DEBUG)
            .then(IndexingProgress::default);
        let span = tracing::Span::current();
        tracing::debug!(target: LOG_TARGET, target_count = targets.len(), "indexing saved declarations and language items");
        let started = Instant::now();
        let index_targets = || {
            db.parallel_map(&targets, |db, &target| {
                let _profile = profile.enter();
                let _entered = progress.as_ref().map(|_| span.enter());
                if let Some(progress) = &progress {
                    progress.start_target(target);
                }
                let result = (|| {
                    let metadata = target_declarations(db, target)?.metadata.value(db)?;
                    // Lang items need final member IDs before the project is published. Preparation
                    // can expand owners and capture includes, so it precedes source validation.
                    if let Some(progress) = &progress {
                        progress.prepare_lang_items(target);
                    }
                    ItemLookupEnv::for_crate(db, target)?;
                    Ok(metadata)
                })();
                if let Some(progress) = &progress {
                    progress.finish_target(target, result.is_ok());
                }
                result
            })
        };
        let declarations = if let Some(progress) = &progress {
            progress.report_while(&workspace, targets.len(), index_targets)
        } else {
            index_targets()
        }?;
        tracing::debug!(
            target: LOG_TARGET,
            elapsed_ms = started.elapsed().as_millis(),
            "finished indexing saved declarations and language items"
        );
        timer.finish();
        // Warm replay can capture inputs while checking old dependencies. Validate the resulting
        // view inside the operation, before Ajika can checkpoint it.
        let timer = metric::SOURCE_VALIDATION.start_timer();
        tracing::debug!(target: LOG_TARGET, "validating saved sources");
        let started = Instant::now();
        SourceInput::validate_saved(&inputs, &db.interner())?;
        tracing::debug!(
            target: LOG_TARGET,
            elapsed_ms = started.elapsed().as_millis(),
            "validated saved sources"
        );
        timer.finish();
        let sources = ProjectSourceIndex::new(
            targets
                .iter()
                .copied()
                .zip(declarations.iter().map(|metadata| metadata.sources.clone())),
        );
        let reports = declarations
            .iter()
            .filter_map(|metadata| metadata.macro_expansion_limit.clone())
            .collect::<Vec<_>>();
        // Ajika may checkpoint or drain pending writes after the callback returns.
        if progress.is_some() {
            finalization_started = Some(Instant::now());
            tracing::debug!(target: LOG_TARGET, "finalizing saved query operation");
        }
        Ok((sources, reports))
    })?;
    operation.finish();

    // These counters cover the indexing run. It can write a checkpoint under pressure or
    // leave the batch for collection once the project is published.
    let stats = database.statistics();
    if let Some(started) = finalization_started {
        tracing::debug!(
            target: LOG_TARGET,
            elapsed_ms = started.elapsed().as_millis(),
            pending_queries = stats.pending_queries,
            bytes_written = stats.bytes_written,
            "finished saved query operation"
        );
    }
    metric::EXECUTED.add(stats.executed);
    metric::VALIDATED.add(stats.validated);
    metric::HITS.add(stats.hits);
    metric::DECODED.add(stats.decoded);
    metric::METADATA_READS.add(stats.metadata_reads);
    metric::PAYLOAD_BYTES_READ.record(ProfileMeasurement::Bytes(stats.bytes_read));
    metric::PAYLOAD_BYTES_WRITTEN.record(ProfileMeasurement::Bytes(stats.bytes_written));
    metric::METADATA_BYTES_WRITTEN.record(ProfileMeasurement::Bytes(stats.metadata_bytes_written));

    // A captured file may belong to a member query that startup has not demanded. Module
    // collection cannot prove that it is detached. The query owner releases its reloadable text.
    // TODO: Retire captures once live input reachability can be proven without eager discovery.
    // Partial declaration ownership cannot prove absence, and expansion also observes files that
    // contribute no declarations. Detached captures can still require stale-source recovery.
    Ok(ProjectState {
        generation_id: ProjectGenerationId::fresh(),
        workspace,
        sources,
        targets,
        macro_expansion_limit_summary: MacroExpansionLimitBuildSummary::capture(&reports),
        reports,
    })
}
