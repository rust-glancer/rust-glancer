//! Coordinates late source discovery requested by macro-generated module declarations.
//!
//! DefMap owns the semantic request, while this project boundary owns source capture, package-local
//! file ids, and ItemTree growth. Each batch is loaded before resuming the retained selected-package
//! construction state. Already collected declarations and expanded macros are not replayed for
//! each newly discovered file.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use anyhow::Context as _;
use rg_def_map::{
    DefMapBuildProgress, DefMapBuildSession, DefMapDb, DefMapReadTxn, GeneratedModuleRequest,
    MacroExpansionPerformancePreference, PackageSlot,
};
use rg_item_tree::ItemTreeDb;
use rg_parse::{FileId, ParseDb};
use rg_text::PackageNameInterners;
use rg_workspace::WorkspaceMetadata;

use crate::profile::metric;
use crate::{ProjectMemoryHooks, ProjectMemoryPurgePoint};

use super::package_set::PhasePackageSet;

// The DefMap session retains its own macro-expansion bound across pauses. This second bound limits
// a valid but pathological chain that discovers one real file per project-owned source wave.
const MAX_GENERATED_MODULE_DISCOVERY_WAVES: usize = 128;

/// Builds selected DefMap packages while allowing generated macros to add source files.
///
/// One retained DefMap session alternates with project-owned source batches until expansion stops
/// asking for new modules. Parse and ItemTree stay mutable for that loop; the returned DefMap no
/// longer contains the requests or continuations used to reach the fixed point.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_packages(
    baseline: &DefMapDb,
    baseline_read: &DefMapReadTxn<'_>,
    workspace: &WorkspaceMetadata,
    parse: &mut ParseDb,
    item_tree: &mut ItemTreeDb,
    packages: &PhasePackageSet,
    names: &mut PackageNameInterners,
    performance_preference: MacroExpansionPerformancePreference,
    memory_hooks: &dyn ProjectMemoryHooks,
) -> anyhow::Result<DefMapDb> {
    // Different requests can resolve to the same package-local file, even in different waves. The
    // session coalesces request identity; this separate map coalesces captured path identity.
    let mut captured_files_by_path = HashMap::<(PackageSlot, PathBuf), FileId>::new();
    let mut wave_count = 0;
    let mut resume_count = 0;
    let mut session = baseline
        .start_package_build(
            baseline_read,
            workspace,
            parse,
            item_tree,
            packages.as_slice(),
            names,
            performance_preference,
        )
        .context("while attempting to start resumable DefMap construction")?;

    loop {
        let resume_timer = (resume_count > 0)
            .then(|| metric::TIMING_GENERATED_MODULE_DEF_MAP_RESUMES.start_timer());
        let progress = session
            .advance(baseline_read, parse, item_tree, names)
            .context("while attempting to continue resumable DefMap construction")?;
        if let Some(timer) = resume_timer {
            timer.finish();
            metric::GENERATED_MODULE_DEF_MAP_RESUMES.inc();
        }

        let requests = match progress {
            DefMapBuildProgress::NeedsGeneratedModules(requests) => requests,
            DefMapBuildProgress::Complete(db) => return Ok(db),
        };
        metric::GENERATED_MODULE_DISCOVERY_WAVES.inc();
        metric::GENERATED_MODULE_REQUESTS.add(requests.len().try_into().unwrap_or(u64::MAX));

        // Do not capture a source beyond the project-owned wave budget. Marking the final batch
        // as missing lets the retained session finish without allocating those modules or
        // publishing files that no semantic module can reach.
        if wave_count == MAX_GENERATED_MODULE_DISCOVERY_WAVES {
            metric::GENERATED_MODULE_DISCOVERY_LIMIT_REACHED.record_bool(true);
            for request in requests {
                session.record_missing_generated_module(request).context(
                    "while attempting to reject a generated module beyond the wave limit",
                )?;
            }
            resume_count += 1;
            continue;
        }

        let loaded_any_source = load_generated_module_sources(
            parse,
            item_tree,
            names,
            packages,
            &mut session,
            requests,
            &mut captured_files_by_path,
        )?;
        if loaded_any_source {
            // Incremental lowering evicts package syntax before returning. Expose the same
            // allocator cleanup boundary as the initial ItemTree pass, once per found wave.
            memory_hooks.purge(ProjectMemoryPurgePoint::AfterItemTreeSyntaxEviction);
        }

        wave_count += 1;
        resume_count += 1;
    }
}

/// Loads one request batch and lowers every encountered source through ordinary ItemTree.
///
/// Missing paths are valid resolutions and remain tracked by the source inventory. A source is
/// recorded on the session only after both Parse capture and context-sensitive ItemTree lowering
/// succeed. The return value says whether at least one request resolved to a real file, so the
/// caller can expose the allocator purge boundary after that wave.
fn load_generated_module_sources(
    parse: &mut ParseDb,
    item_tree: &mut ItemTreeDb,
    names: &mut PackageNameInterners,
    source_packages: &PhasePackageSet,
    session: &mut DefMapBuildSession,
    requests: Vec<GeneratedModuleRequest>,
    captured_files_by_path: &mut HashMap<(PackageSlot, PathBuf), FileId>,
) -> anyhow::Result<bool> {
    let sources = parse.source_inventory_handle();
    let mut touched_packages = HashSet::new();
    let mut loaded_any_source = false;

    for request in requests {
        anyhow::ensure!(
            source_packages.contains(request.package()),
            "cache-backed package {} emitted a generated module source request",
            request.package().0,
        );
        metric::GENERATED_MODULE_UNIQUE_REQUESTS.inc();

        // The request context belongs to the module containing `mod child;`. Resolution returns
        // the distinct child context that must be used to lower the selected file.
        let Some(resolution) = request
            .parent_context()
            .resolve_module_name(&sources, request.module_name(), request.path_override())
            .with_context(|| {
                format!(
                    "while attempting to resolve generated module {} for package {}",
                    request.module_name(),
                    request.package().0,
                )
            })?
        else {
            session
                .record_missing_generated_module(request)
                .context("while attempting to record a missing generated module")?;
            metric::GENERATED_MODULE_MISSING_PATHS.inc();
            continue;
        };
        let (path, child_context) = resolution.into_parts();
        let child_context = Arc::new(child_context);

        let package_slot = request.package();
        let requested_path_key = (package_slot, path.clone());
        let previous_file_count = parse
            .package(package_slot.0)
            .expect("source package should exist while loading module request")
            .parsed_files()
            .count();
        let file_id = if let Some(file_id) =
            captured_files_by_path.get(&requested_path_key).copied()
        {
            metric::GENERATED_MODULE_COALESCED_PATHS.inc();
            file_id
        } else {
            let file_id = parse
                .package_mut(package_slot.0)
                .expect("source package should exist while capturing module request")
                .parse_file(&sources, &path)
                .with_context(|| {
                    format!(
                        "while attempting to capture generated module source {}",
                        path.display()
                    )
                })?;
            let canonical_path = parse
                .package(package_slot.0)
                .and_then(|package| package.file_path(file_id))
                .expect("captured generated module source should have a canonical path")
                .to_path_buf();
            let canonical_path_key = (package_slot, canonical_path);

            // `parse_file` can canonicalize another spelling onto a file captured earlier. Keep
            // both spellings, while treating the canonical identity as one discovered path.
            if let Some(existing_file_id) = captured_files_by_path.get(&canonical_path_key).copied()
            {
                debug_assert_eq!(existing_file_id, file_id);
                captured_files_by_path.insert(requested_path_key, existing_file_id);
                metric::GENERATED_MODULE_COALESCED_PATHS.inc();
                existing_file_id
            } else {
                captured_files_by_path.insert(requested_path_key, file_id);
                captured_files_by_path.insert(canonical_path_key, file_id);
                metric::GENERATED_MODULE_UNIQUE_PATHS.inc();
                file_id
            }
        };
        touched_packages.insert(package_slot);

        // A shared file can contribute modules from more than one logical context. Its FileTree is
        // reused, but contextual source edges still need to be traversed for this request.
        let lowering = item_tree
            .lower_package_file(
                parse,
                package_slot.0,
                file_id,
                child_context.as_ref().clone(),
                names,
            )
            .with_context(|| {
                format!(
                    "while attempting to lower generated module source {}",
                    path.display()
                )
            })?;
        metric::GENERATED_MODULE_ITEM_TREE_FILES_LOWERED
            .add(lowering.newly_lowered_files.try_into().unwrap_or(u64::MAX));
        metric::GENERATED_MODULE_ITEM_TREE_FILES_REUSED
            .add(lowering.reused_files.try_into().unwrap_or(u64::MAX));

        let current_file_count = parse
            .package(package_slot.0)
            .expect("source package should exist after module lowering")
            .parsed_files()
            .count();
        metric::GENERATED_MODULE_DISCOVERED_FILES.add(
            current_file_count
                .saturating_sub(previous_file_count)
                .try_into()
                .unwrap_or(u64::MAX),
        );
        session
            .record_generated_module(request, file_id, child_context)
            .context("while attempting to record a loaded generated module")?;
        loaded_any_source = true;
    }

    // A coalesced request may have rehydrated syntax after the first request already lowered and
    // evicted the file. Keep the same package-level eviction boundary as ordinary ItemTree work.
    for package_slot in touched_packages {
        parse
            .package_mut(package_slot.0)
            .expect("touched source package should remain present")
            .evict_syntax_trees();
    }

    Ok(loaded_any_source)
}
