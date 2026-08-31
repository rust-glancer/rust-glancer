//! Builds a project in package batches.
//!
//! Parsing and cache probing still happen once for the complete project. After that, a small group
//! of packages runs through Item Tree, DefMap, Semantic IR, and configured Body IR. Finished
//! packages are written and released when their residency policy allows it, then the next group
//! starts. This avoids keeping incomplete phase data for every package in memory at once.
//!
//! Dependencies are put in the same batch or an earlier batch. One cache transaction spans all
//! batches, so later packages can read artifacts written by earlier ones. If the build fails, the
//! incomplete-update marker still makes the next process discard the package set as one unit.

mod schedule;

use std::time::Instant;

use anyhow::Context as _;
use rg_body_ir::{BodyIrBuildPolicy, BodyIrDb};
use rg_def_map::DefMapDb;
use rg_item_tree::ItemTreeDb;
use rg_package_store::{PackageEntry, PackageStore};
use rg_parse::ParseDb;
use rg_semantic_ir::SemanticIrDb;
use rg_std::Shrink;
use rg_text::PackageNameInterners;
use rg_workspace::WorkspaceMetadata;

use self::schedule::PackageBatchSchedule;

use crate::{
    IndexingPerformancePreference, PackageBatchSize, PackageResidency, PackageResidencyPlan,
    cache::{PackageCacheStore, WorkspaceCachePlan},
    memory::{ProjectMemoryHooks, ProjectMemoryPurgePoint},
    profile::{BuildMemorySampler, metric},
    project::{
        SplitIndexingMode, StartupCacheLoad,
        loading::PackageReadLoaders,
        macro_source_files,
        package_artifacts::{PackageArtifactPhases, PackageArtifactWriter},
        stats::MacroExpansionLimitBuildSummary,
    },
};

use super::{
    checkpoint_memory::{CheckpointMemory, checkpoint_memory},
    phases::{BuiltPhases, PackageBuildPlan},
};

/// Builds complete package artifacts one package batch at a time.
#[allow(clippy::too_many_arguments)]
pub(super) fn build(
    workspace: &WorkspaceMetadata,
    body_ir_policy: BodyIrBuildPolicy,
    indexing_preference: IndexingPerformancePreference,
    package_residency: &PackageResidencyPlan,
    cache_plan: &WorkspaceCachePlan,
    cache_store: &PackageCacheStore,
    startup_cache_load: StartupCacheLoad,
    split_indexing_mode: SplitIndexingMode,
    memory_hooks: &dyn ProjectMemoryHooks,
    sampler: &mut BuildMemorySampler,
    batch_size: PackageBatchSize,
) -> anyhow::Result<BuiltPhases> {
    // ---------------------
    // 1. Parse all packages
    // ---------------------
    let mut parse = ParseDb::build(workspace).context("while attempting to build parse db")?;
    checkpoint_memory!(parse).checkpoint(sampler, metric::PARSE_MEMORY, &parse);

    // -------------------------------
    // 2. Choose and schedule source work
    // -------------------------------
    let mut build_plan = PackageBuildPlan::build(
        startup_cache_load,
        body_ir_policy,
        package_residency,
        cache_plan,
        cache_store,
        workspace,
        &mut parse,
    );
    checkpoint_memory!(parse, build_plan).checkpoint(
        sampler,
        metric::CACHE_PROBE_MEMORY,
        &build_plan,
    );

    let schedule = PackageBatchSchedule::build(workspace, &build_plan.source_packages, batch_size);
    tracing::info!(
        source_packages = build_plan.source_packages.as_slice().len(),
        batches = schedule.batch_count(),
        largest_batch = schedule.largest_batch_size(),
        cycle_blocked_packages = schedule.cycle_blocked_package_count,
        package_batch_size = batch_size.get(),
        split_indexing_mode = ?split_indexing_mode,
        "batch indexing planned"
    );
    if schedule.cycle_blocked_package_count > 0 {
        tracing::info!(
            packages = schedule.cycle_blocked_package_count,
            "packages blocked by a dependency cycle will be indexed in one batch"
        );
    }

    let mut names = PackageNameInterners::new(parse.package_count());
    let mut source_fingerprints = cache_plan
        .source_fingerprints(workspace.workspace_root(), &parse)
        .context("while attempting to compute package cache source fingerprints")?;
    checkpoint_memory!(names, parse, build_plan, source_fingerprints).checkpoint(
        sampler,
        metric::CACHE_SOURCE_FINGERPRINTS_MEMORY,
        &source_fingerprints,
    );

    // Every retained phase begins with the same package-slot shape. A completed batch replaces its
    // selected slots, and offloadable replacements return to lazy markers after their joint
    // artifact has been written.
    let mut def_map =
        DefMapDb::from_offloaded_manifests(std::mem::take(&mut build_plan.def_map_manifests));
    let mut semantic_ir =
        SemanticIrDb::from_package_store(PackageStore::all_offloaded(parse.package_count()));
    let body_ir_entries = std::mem::take(&mut build_plan.body_ir_coverage)
        .into_iter()
        .map(PackageEntry::offloaded_with)
        .collect();
    let mut body_ir = BodyIrDb::from_package_store(PackageStore::from_entries(body_ir_entries));
    let mut macro_expansion_limit_summary = MacroExpansionLimitBuildSummary::default();

    let has_offloadable_source_packages = build_plan
        .source_packages
        .iter()
        .any(|package| package_residency.package(package) == Some(PackageResidency::Offloadable));
    let cache_update = if has_offloadable_source_packages {
        Some(
            cache_store
                .begin_artifact_update()
                .context("while attempting to begin batch indexing cache update")?,
        )
    } else {
        None
    };
    let mut pending_packages = build_plan.source_packages.as_slice().to_vec();

    // --------------------------------------------
    // 3. Build, persist, and release each package batch
    // --------------------------------------------
    for (batch_index, batch) in schedule.batches.iter().enumerate() {
        let batch_started = Instant::now();
        tracing::info!(
            batch = batch_index + 1,
            batches = schedule.batch_count(),
            packages = batch.as_slice().len(),
            "batch indexing started"
        );

        let loaders = PackageReadLoaders::from_cache_excluding(
            cache_plan.clone(),
            cache_store.clone(),
            source_fingerprints.clone(),
            &pending_packages,
        );
        let rebuild_subset = batch.visible_dependency_subset(workspace);
        let resident_packages = package_residency.resident_packages(batch.as_slice());

        // Item trees are the largest purely transient cross-phase input. Restricting them to one
        // package batch removes the global ItemTree/DefMap overlap before any durable phase changes.
        let package_indices = batch.package_indices();
        let mut item_tree = ItemTreeDb::build_packages(&mut parse, &package_indices, &mut names)
            .context("while attempting to build package batch item trees")?;
        checkpoint_memory!(
            names,
            parse,
            build_plan,
            item_tree,
            source_fingerprints,
            def_map,
            semantic_ir,
            body_ir,
        )
        .checkpoint(sampler, metric::ITEM_TREE_MEMORY, &item_tree);

        parse.evict_syntax_trees();
        parse.shrink_to_fit();
        checkpoint_memory!(
            names,
            parse,
            build_plan,
            item_tree,
            source_fingerprints,
            def_map,
            semantic_ir,
            body_ir,
        )
        .checkpoint(sampler, metric::ITEM_TREE_SYNTAX_EVICTION_MEMORY, &parse);

        let baseline_def_map_txn =
            def_map.read_txn_for_subset(loaders.def_map.clone(), &rebuild_subset);
        let next_def_map_output = macro_source_files::build_packages(
            &def_map,
            &baseline_def_map_txn,
            workspace,
            &mut parse,
            &mut item_tree,
            batch,
            &resident_packages,
            &mut names,
            indexing_preference.macro_expansion_preference(),
            memory_hooks,
        )
        .context("while attempting to build package batch def maps")?;
        drop(baseline_def_map_txn);
        let (next_def_map, generated_items) = next_def_map_output.into_parts();
        def_map = next_def_map;

        macro_expansion_limit_summary.extend(MacroExpansionLimitBuildSummary::capture(
            &def_map,
            batch.as_slice(),
        ));

        // No later package can discover a source inside this completed package: generated modules
        // and includes are owned by the package whose DefMap is being built. Its final fingerprint
        // can therefore make the newly written artifact visible to subsequent dependency reads.
        let provisional_fingerprints = batch
            .iter()
            .map(|package| {
                (
                    package,
                    source_fingerprints.get(package.0).copied().flatten(),
                )
            })
            .collect::<Vec<_>>();
        cache_plan
            .refresh_source_fingerprints(
                workspace.workspace_root(),
                &parse,
                &mut source_fingerprints,
                batch.as_slice(),
            )
            .context("while attempting to finalize package batch source fingerprints")?;
        let changed_fingerprint_count = provisional_fingerprints
            .into_iter()
            .filter(|(package, provisional)| {
                source_fingerprints.get(package.0).copied().flatten() != *provisional
            })
            .count();
        metric::MACRO_SOURCE_FILE_CACHE_FINGERPRINT_CHANGES
            .add(changed_fingerprint_count.try_into().unwrap_or(u64::MAX));
        checkpoint_memory!(
            names,
            parse,
            build_plan,
            item_tree,
            source_fingerprints,
            def_map,
            semantic_ir,
            body_ir,
            generated_items,
        )
        .checkpoint(sampler, metric::DEF_MAP_MEMORY, &def_map);

        semantic_ir = semantic_ir
            .build_packages(
                &item_tree,
                &def_map,
                &generated_items,
                batch.as_slice(),
                loaders.def_map.clone(),
                loaders.semantic_ir.clone(),
                &rebuild_subset,
            )
            .context("while attempting to build package batch semantic IR")?;
        checkpoint_memory!(
            names,
            parse,
            build_plan,
            item_tree,
            source_fingerprints,
            def_map,
            semantic_ir,
            body_ir,
            generated_items,
        )
        .checkpoint(sampler, metric::SEMANTIC_IR_MEMORY, &semantic_ir);

        drop(generated_items);
        drop(item_tree);
        checkpoint_memory!(
            names,
            parse,
            build_plan,
            source_fingerprints,
            def_map,
            semantic_ir,
            body_ir,
        )
        .checkpoint_without_retained(sampler, metric::ITEM_TREE_DROP_MEMORY);

        // A package must have one complete artifact before its declaration payloads can be
        // released. Batch indexing therefore includes configured Body IR before finishing the
        // batch, even when the project would otherwise start early. Lower peak memory delays the
        // queryable project boundary rather than retaining a second global Body IR pass.
        body_ir = body_ir
            .builder(
                &parse,
                &def_map,
                &semantic_ir,
                batch.as_slice(),
                &resident_packages,
                &mut names,
                loaders.def_map,
                loaders.semantic_ir,
                &rebuild_subset,
            )
            .worker_limit(indexing_preference.body_ir_worker_limit())
            .configured_bodies(body_ir_policy)
            .build()
            .context("while attempting to build package batch body IR")?;
        parse.evict_syntax_trees();
        checkpoint_memory!(
            names,
            parse,
            build_plan,
            source_fingerprints,
            def_map,
            semantic_ir,
            body_ir,
        )
        .checkpoint(sampler, metric::BODY_IR_MEMORY, &body_ir);

        let offloadable_batch = batch.filter(|package| {
            package_residency.package(package) == Some(PackageResidency::Offloadable)
        });
        if !offloadable_batch.is_empty() {
            let update = cache_update
                .as_ref()
                .context("batch indexing cache update should exist for offloadable packages")?;
            PackageArtifactWriter::new(
                cache_plan,
                cache_store,
                &source_fingerprints,
                &parse,
                &def_map,
                &semantic_ir,
                &body_ir,
            )
            .write_packages(update, offloadable_batch.as_slice())
            .context("while attempting to write package batch artifacts")?;
            checkpoint_memory!(
                names,
                parse,
                build_plan,
                source_fingerprints,
                def_map,
                semantic_ir,
                body_ir,
            )
            .checkpoint_labeled(sampler, "after package batch cache write");

            let mut artifact_phases =
                PackageArtifactPhases::new(&mut def_map, &mut semantic_ir, &mut body_ir);
            for package in offloadable_batch.iter() {
                artifact_phases.offload_package(package).with_context(|| {
                    format!(
                        "while attempting to offload package batch package {}",
                        package.0
                    )
                })?;
            }
            parse.offload_line_indexes_for_packages(&offloadable_batch.package_indices());
            Shrink::shrink_to_fit(&mut names);
            memory_hooks.purge(ProjectMemoryPurgePoint::AfterPackageRebuild);
            checkpoint_memory!(
                names,
                parse,
                build_plan,
                source_fingerprints,
                def_map,
                semantic_ir,
                body_ir,
            )
            .checkpoint_labeled(sampler, "after package batch offload");
        }

        pending_packages.retain(|package| !batch.contains(*package));
        tracing::info!(
            batch = batch_index + 1,
            batches = schedule.batch_count(),
            packages = batch.as_slice().len(),
            remaining_packages = pending_packages.len(),
            elapsed_ms = batch_started.elapsed().as_millis(),
            "batch indexing finished"
        );
    }

    // ---------------------------------
    // 4. Validate and publish durability
    // ---------------------------------
    parse.seal_sources();
    parse
        .validate_saved_sources()
        .context("while attempting to validate batch indexing source generation")?;
    if let Some(update) = cache_update {
        update
            .commit()
            .context("while attempting to commit batch indexing cache update")?;
    }
    cache_store
        .cleanup_stale_generations()
        .context("while attempting to clean stale package cache generations")?;
    drop(build_plan);

    // --------------------------
    // 5. Compact retained state
    // --------------------------
    parse.evict_syntax_trees();
    parse.evict_saved_source_text();
    parse.shrink_to_fit();
    Shrink::shrink_to_fit(&mut names);
    checkpoint_memory!(
        names,
        parse,
        source_fingerprints,
        def_map,
        semantic_ir,
        body_ir,
    )
    .checkpoint(sampler, metric::PARSE_SYNTAX_EVICTION_MEMORY, &parse);

    Ok(BuiltPhases {
        package_source_fingerprints: source_fingerprints,
        names,
        parse,
        macro_expansion_limit_summary,
        def_map,
        semantic_ir,
        body_ir,
    })
}
