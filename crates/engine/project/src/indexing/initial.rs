//! Assembles the initial project state from its workspace and configured indexing phases.

use std::sync::Arc;

use anyhow::Context as _;
use rg_body_ir::BodyIrBuildPolicy;
use rg_workspace::{CargoMetadataConfig, WorkspaceLoweringConfig, WorkspaceMetadata};

use super::phases;
use crate::{
    IndexingPerformancePreference, PackageBatchSize, PackageResidencyPlan, PackageResidencyPolicy,
    ProjectMemoryHooks, SplitIndexingMode, StartupCacheLoad,
    profile::{BuildMemorySampler, metric},
    state::{ProjectGenerationId, ProjectState},
    storage::cache::{PackageCacheInstance, PackageCacheStore, WorkspaceCachePlan},
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_resident_state(
    workspace: WorkspaceMetadata,
    workspace_lowering_config: WorkspaceLoweringConfig,
    cargo_metadata_config: CargoMetadataConfig,
    cache_instance: PackageCacheInstance,
    body_ir_policy: BodyIrBuildPolicy,
    split_indexing_mode: SplitIndexingMode,
    indexing_preference: IndexingPerformancePreference,
    package_batch_size: PackageBatchSize,
    package_residency_policy: PackageResidencyPolicy,
    startup_cache_load: StartupCacheLoad,
    memory_hooks: Arc<dyn ProjectMemoryHooks>,
    memory_sampler: &mut BuildMemorySampler,
) -> anyhow::Result<ProjectState> {
    // Workspace lowering already scanned passive Cargo build outputs because recovered cfg and
    // compile-time environment must be present before Parse and ItemTree. Replay its bounded totals
    // into the project profile here; the scan itself is not repeated during project construction.
    let cargo_build_outputs = workspace.cargo_build_output_stats();
    metric::CARGO_BUILD_OUTPUT_TARGET_DIRECTORIES.add(
        cargo_build_outputs
            .target_directories()
            .try_into()
            .unwrap_or(u64::MAX),
    );
    metric::CARGO_BUILD_OUTPUT_DEPS_DIRECTORIES.add(
        cargo_build_outputs
            .deps_directories()
            .try_into()
            .unwrap_or(u64::MAX),
    );
    metric::CARGO_BUILD_OUTPUT_DEP_INFO_FILES.add(
        cargo_build_outputs
            .dep_info_files()
            .try_into()
            .unwrap_or(u64::MAX),
    );
    metric::CARGO_BUILD_OUTPUT_BUILD_SCRIPT_PACKAGES.add(
        cargo_build_outputs
            .build_script_packages()
            .try_into()
            .unwrap_or(u64::MAX),
    );
    metric::CARGO_BUILD_OUTPUT_MATCHED_RUSTC_UNITS.add(
        cargo_build_outputs
            .matched_rustc_units()
            .try_into()
            .unwrap_or(u64::MAX),
    );
    metric::CARGO_BUILD_OUTPUT_CANDIDATES.add(
        cargo_build_outputs
            .build_output_candidates()
            .try_into()
            .unwrap_or(u64::MAX),
    );
    metric::CARGO_BUILD_OUTPUT_SELECTED_PACKAGES.add(
        cargo_build_outputs
            .selected_packages()
            .try_into()
            .unwrap_or(u64::MAX),
    );
    metric::CARGO_BUILD_OUTPUT_GENERATED_FILES.add(
        cargo_build_outputs
            .generated_files()
            .try_into()
            .unwrap_or(u64::MAX),
    );
    metric::CARGO_BUILD_OUTPUT_GENERATED_BYTES.add(cargo_build_outputs.generated_bytes());
    metric::CARGO_BUILD_OUTPUT_SCAN.record(cargo_build_outputs.scan_duration());

    let package_residency = PackageResidencyPlan::build(&workspace, package_residency_policy);
    let cache_plan = WorkspaceCachePlan::build(&workspace);
    let cache_store =
        PackageCacheStore::for_instance(&cache_plan, package_residency_policy, &cache_instance);
    cache_store
        .recover_incomplete_update()
        .context("while attempting to recover an incomplete package cache update")?;
    let phases = phases::build(
        &workspace,
        body_ir_policy,
        indexing_preference,
        package_batch_size,
        &package_residency,
        &cache_plan,
        &cache_store,
        startup_cache_load,
        split_indexing_mode,
        memory_hooks.as_ref(),
        memory_sampler,
    )?;

    Ok(ProjectState {
        generation_id: ProjectGenerationId::fresh(),
        workspace,
        workspace_lowering_config,
        cargo_metadata_config,
        cache_plan,
        cache_instance,
        cache_store,
        package_source_fingerprints: phases.package_source_fingerprints,
        body_ir_policy,
        split_indexing_mode,
        indexing_preference,
        package_batch_size,
        package_residency_policy,
        package_residency,
        memory_hooks,
        names: phases.names,
        parse: Arc::new(phases.parse),
        macro_expansion_limit_summary: phases.macro_expansion_limit_summary,
        def_map: phases.def_map,
        semantic_ir: phases.semantic_ir,
        body_ir: phases.body_ir,
    })
}
