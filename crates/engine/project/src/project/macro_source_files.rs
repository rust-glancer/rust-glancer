//! Loads real files that become reachable only through expanded macro syntax.
//!
//! The ordinary ItemTree pass follows files visible in source, but a macro expansion can reveal a
//! new edge only after DefMap has started. The two supported edges have different Rust semantics:
//!
//! ```text
//! macro output: mod generated;                       // collect file in a new child module
//! macro output: include!(concat!(env!("OUT_DIR"),    // splice file into the caller's module
//!                                "/bindings.rs"));
//! ```
//!
//! DefMap owns those meanings and pauses with [`MacroSourceFileRequest`] values. This module owns
//! the filesystem side: it resolves a path, captures the file into Parse, lowers it into ItemTree
//! with the requested module context, and records exactly one found-or-missing answer. It then
//! resumes the retained DefMap session instead of rebuilding declarations and re-expanding macros
//! that were already settled before the request.
//!
//! More files can expose more macro calls, so one request batch can produce another. The loop below
//! continues until DefMap reaches a fixed point; its wave limit bounds pathological projects rather
//! than defining ordinary control flow.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use anyhow::Context as _;
use rg_def_map::{
    DefMapBuildOutput, DefMapBuildProgress, DefMapBuildSession, DefMapDb, DefMapReadTxn,
    MacroExpansionPerformancePreference, MacroSourceFileRequest, PackageSlot,
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
const MAX_MACRO_SOURCE_FILE_DISCOVERY_WAVES: usize = 128;

/// Builds selected DefMap packages while allowing expanded syntax to add real source files.
///
/// One retained DefMap session alternates with project-owned file batches until expansion stops
/// asking for more files. Parse and ItemTree stay mutable for that loop; the returned DefMap no
/// longer contains the requests or continuations used to reach the fixed point.
///
/// `packages` selects the payloads rebuilt by the session. `copy_compact_packages` is the subset
/// whose frozen DefMaps will remain resident afterward. Keeping those inputs separate avoids making
/// a compact second copy of a package that the project will serialize and offload immediately.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_packages(
    baseline: &DefMapDb,
    baseline_read: &DefMapReadTxn<'_>,
    workspace: &WorkspaceMetadata,
    parse: &mut ParseDb,
    item_tree: &mut ItemTreeDb,
    packages: &PhasePackageSet,
    copy_compact_packages: &[PackageSlot],
    names: &mut PackageNameInterners,
    performance_preference: MacroExpansionPerformancePreference,
    memory_hooks: &dyn ProjectMemoryHooks,
) -> anyhow::Result<DefMapBuildOutput> {
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
            copy_compact_packages,
            names,
            performance_preference,
        )
        .context("while attempting to start resumable DefMap construction")?;

    loop {
        // Advance owns all semantic work. It either freezes the finished DefMap or hands this
        // project boundary a complete batch that must be answered before another advance.
        let resume_timer = (resume_count > 0)
            .then(|| metric::TIMING_MACRO_SOURCE_FILE_DEF_MAP_RESUMES.start_timer());
        let progress = session
            .advance(baseline_read, parse, item_tree, names)
            .context("while attempting to continue resumable DefMap construction")?;
        if let Some(timer) = resume_timer {
            timer.finish();
            metric::MACRO_SOURCE_FILE_DEF_MAP_RESUMES.inc();
        }

        let requests = match progress {
            DefMapBuildProgress::NeedsMacroSourceFiles(requests) => requests,
            DefMapBuildProgress::Complete(output) => return Ok(output),
        };
        metric::MACRO_SOURCE_FILE_DISCOVERY_WAVES.inc();
        metric::MACRO_SOURCE_FILE_REQUESTS.add(requests.len().try_into().unwrap_or(u64::MAX));

        // Do not capture a source beyond the project-owned wave budget. Marking the final batch
        // as missing lets the retained session finish without publishing files that no pending
        // semantic operation can consume.
        if wave_count == MAX_MACRO_SOURCE_FILE_DISCOVERY_WAVES {
            metric::MACRO_SOURCE_FILE_DISCOVERY_LIMIT_REACHED.record_bool(true);
            for request in requests {
                session.record_missing_macro_source_file(request).context(
                    "while attempting to reject a macro source file beyond the wave limit",
                )?;
            }
            resume_count += 1;
            continue;
        }

        // Every request is consumed into one found-or-missing answer on `session`. Recording the
        // whole batch is what makes the next `advance` safe: the session rejects forgotten and
        // duplicate answers rather than continuing with a partial source graph.
        let loaded_any_file = load_macro_source_files(
            parse,
            item_tree,
            names,
            packages,
            &mut session,
            requests,
            &mut captured_files_by_path,
        )?;
        if loaded_any_file {
            // Incremental lowering evicts package syntax before returning. Expose the same
            // allocator cleanup boundary as the initial ItemTree pass, once per found wave.
            memory_hooks.purge(ProjectMemoryPurgePoint::AfterItemTreeSyntaxEviction);
        }

        wave_count += 1;
        resume_count += 1;
    }
}

/// Turns one complete request batch into found-or-missing answers for the retained session.
///
/// A found file is recorded only after Parse capture and context-sensitive ItemTree lowering
/// succeed. Different requests may share one physical `FileId`, but each request is lowered with
/// its own logical module context: the same file can legally be interpreted at two call sites.
/// Missing paths are completed answers and remain tracked by the source inventory. The return
/// value only tells the caller whether this wave created real lowering work and therefore reached
/// an allocator purge boundary.
fn load_macro_source_files(
    parse: &mut ParseDb,
    item_tree: &mut ItemTreeDb,
    names: &mut PackageNameInterners,
    source_packages: &PhasePackageSet,
    session: &mut DefMapBuildSession,
    requests: Vec<MacroSourceFileRequest>,
    captured_files_by_path: &mut HashMap<(PackageSlot, PathBuf), FileId>,
) -> anyhow::Result<bool> {
    let sources = parse.source_inventory_handle();
    let mut touched_packages = HashSet::new();
    let mut loaded_any_file = false;

    for request in requests {
        anyhow::ensure!(
            source_packages.contains(request.package()),
            "cache-backed package {} emitted a macro source-file request",
            request.package().0,
        );
        metric::MACRO_SOURCE_FILE_UNIQUE_REQUESTS.inc();

        let resolution = match &request {
            MacroSourceFileRequest::Module {
                parent_context,
                module_name,
                path_override,
                ..
            } => parent_context
                .resolve_module_name(&sources, module_name, path_override.as_deref())
                .with_context(|| {
                    format!(
                        "while attempting to resolve generated module {module_name} for package {}",
                        request.package().0,
                    )
                })?
                .map(|resolution| {
                    let (path, child_context) = resolution.into_parts();
                    let child_context = Arc::new(child_context);
                    (
                        path,
                        Arc::clone(&child_context),
                        MacroSourceFileKind::Module { child_context },
                    )
                }),
            MacroSourceFileRequest::Include {
                origin_file,
                module_file_context,
                path,
                ..
            } => {
                let package = parse
                    .package(request.package().0)
                    .expect("source package should exist while resolving generated include");
                let current_file = package.parsed_file(*origin_file).with_context(|| {
                    format!("while attempting to fetch generated include origin {origin_file:?}")
                })?;
                path.resolve(&current_file, package.cargo_generated_sources())
                    .map(|path| {
                        (
                            path,
                            Arc::clone(module_file_context),
                            MacroSourceFileKind::Include,
                        )
                    })
            }
        };
        let Some((path, lowering_context, file_kind)) = resolution else {
            session
                .record_missing_macro_source_file(request)
                .context("while attempting to record a missing macro source file")?;
            metric::MACRO_SOURCE_FILE_MISSING_PATHS.inc();
            continue;
        };

        let package_slot = request.package();
        let requested_path_key = (package_slot, path.clone());
        let previous_file_count = parse
            .package(package_slot.0)
            .expect("source package should exist while loading a macro source file")
            .parsed_files()
            .count();
        let file_id = if let Some(file_id) =
            captured_files_by_path.get(&requested_path_key).copied()
        {
            metric::MACRO_SOURCE_FILE_COALESCED_PATHS.inc();
            file_id
        } else {
            let file_id = match parse
                .package_mut(package_slot.0)
                .expect("source package should exist while capturing a macro source file")
                .parse_file(&sources, &path)
            {
                Ok(file_id) => file_id,
                // Build outputs are optional historical evidence and can disappear after a Cargo
                // clean. Treat that include as absent instead of making otherwise valid source
                // analysis fail. Generated module loading keeps its existing hard-error behavior
                // below once module resolution has selected a source path.
                Err(_) if matches!(&file_kind, MacroSourceFileKind::Include) => {
                    session
                        .record_missing_macro_source_file(request)
                        .context("while attempting to record an unreadable generated include")?;
                    metric::MACRO_SOURCE_FILE_MISSING_PATHS.inc();
                    continue;
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "while attempting to capture generated module source {}",
                            path.display()
                        )
                    });
                }
            };
            let canonical_path = parse
                .package(package_slot.0)
                .and_then(|package| package.file_path(file_id))
                .expect("captured macro source file should have a canonical path")
                .to_path_buf();
            let canonical_path_key = (package_slot, canonical_path);

            // `parse_file` can canonicalize another spelling onto a file captured earlier. Keep
            // both spellings, while treating the canonical identity as one discovered path.
            if let Some(existing_file_id) = captured_files_by_path.get(&canonical_path_key).copied()
            {
                debug_assert_eq!(existing_file_id, file_id);
                captured_files_by_path.insert(requested_path_key, existing_file_id);
                metric::MACRO_SOURCE_FILE_COALESCED_PATHS.inc();
                existing_file_id
            } else {
                captured_files_by_path.insert(requested_path_key, file_id);
                captured_files_by_path.insert(canonical_path_key, file_id);
                metric::MACRO_SOURCE_FILE_UNIQUE_PATHS.inc();
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
                lowering_context.as_ref().clone(),
                names,
            )
            .with_context(|| {
                format!(
                    "while attempting to lower macro source file {}",
                    path.display()
                )
            })?;
        metric::MACRO_SOURCE_FILE_ITEM_TREE_FILES_LOWERED
            .add(lowering.newly_lowered_files.try_into().unwrap_or(u64::MAX));
        metric::MACRO_SOURCE_FILE_ITEM_TREE_FILES_REUSED
            .add(lowering.reused_files.try_into().unwrap_or(u64::MAX));

        let current_file_count = parse
            .package(package_slot.0)
            .expect("source package should exist after module lowering")
            .parsed_files()
            .count();
        metric::MACRO_SOURCE_FILE_DISCOVERED_FILES.add(
            current_file_count
                .saturating_sub(previous_file_count)
                .try_into()
                .unwrap_or(u64::MAX),
        );
        match file_kind {
            MacroSourceFileKind::Module { child_context } => session
                .record_module_file(request, file_id, child_context)
                .context("while attempting to record a loaded module file")?,
            MacroSourceFileKind::Include => session
                .record_include_file(request, file_id)
                .context("while attempting to record a loaded include file")?,
        }
        loaded_any_file = true;
    }

    // A coalesced request may have rehydrated syntax after the first request already lowered and
    // evicted the file. Keep the same package-level eviction boundary as ordinary ItemTree work.
    for package_slot in touched_packages {
        parse
            .package_mut(package_slot.0)
            .expect("touched source package should remain present")
            .evict_syntax_trees();
    }

    Ok(loaded_any_file)
}

enum MacroSourceFileKind {
    Module {
        child_context: Arc<rg_parse::ModuleFileContext>,
    },
    Include,
}
