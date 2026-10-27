//! Builds module scopes by alternating import resolution and item macro expansion.
//!
//! Imports can expose a macro, and that macro can emit more imports or declarations. The outer
//! loop settles imports; the inner loop expands the calls made available by direct declarations.
//! Real files discovered by expansion are loaded and collected before that inner queue continues.
//! Runtime caches, scan cursors, and the expansion budget live across the complete construction.

use anyhow::Context as _;
use rg_ir_model::{ModuleRef, PackageSlot, Path};
use rg_item_tree::ItemTreeDb;
use rg_macro_runtime::{MacroExpansionPerformancePreference, MacroExpansionRuntime};
use rg_parse::{Package, ParseDb};
use rg_text::PackageNameInterners;
use rg_workspace::{PackageOrigin, SysrootCrate, WorkspaceMetadata};

use super::{
    MacroSourceFileResolutions,
    crates::CrateBuildStore,
    imports::{ImportResolutionExecutor, ImportWorklist},
    macros::{
        BuiltinDeriveExpansion, MAX_MACRO_EXPANSION_PASSES, MacroExpansionCursors,
        MacroExpansionScan, apply_expansion_attempts, apply_pending_macro_source_files,
        collect_expansion_attempts, expand_expansion_attempts,
        mark_retryable_macros_skipped_by_limit,
    },
    resolution_env::BuildResolutionEnv,
};
use crate::{
    DefMapReadTxn, MacroSourceFileLoader, MacroSourceFileRequest, MacroSourceFileResolution,
    ScopeResolver,
    profile::{ImportResolutionPassMetrics, metric, record_import_resolution_pass},
};

/// Selects the standard prelude module visible from each dirty crate.
///
/// The prelude path depends on the crate's edition, and the module it resolves to can live in an
/// unselected package. Resolution therefore uses the same fresh-state-plus-baseline environment
/// as the later import fixed point.
pub(crate) fn select_preludes(
    baseline: &DefMapReadTxn<'_>,
    workspace: &WorkspaceMetadata,
    packages: &[Package],
    crates: &mut CrateBuildStore,
    interners: &mut PackageNameInterners,
) -> anyhow::Result<()> {
    // No imports have run yet, so module scopes contain only direct declarations. Apply the
    // proc-macro export boundary before looking up preludes across crates.
    crates.censor_proc_macro_exports();
    let env = BuildResolutionEnv::new(baseline, crates);

    // Store selected preludes out-of-band first so path resolution can borrow all crate builders
    // immutably while we inspect roots across packages.
    let mut selected_preludes = packages
        .iter()
        .enumerate()
        .map(|(package_slot, _)| {
            crates
                .package(PackageSlot(package_slot))
                .map(|crates| vec![None; crates.len()])
        })
        .collect::<Vec<_>>();

    for (package_slot, package) in packages.iter().enumerate() {
        let Some(package_states) = crates.package(PackageSlot(package_slot)) else {
            continue;
        };
        let workspace_package = workspace.package(package.id()).with_context(|| {
            format!(
                "while attempting to fetch workspace metadata for package {}",
                package.id()
            )
        })?;
        let interner = interners.package_mut(package_slot).with_context(|| {
            format!("while attempting to fetch name interner for package {package_slot}")
        })?;
        // Each crate resolves its edition prelude from its own root.
        for (crate_slot, state) in package_states.iter().enumerate() {
            let mut prelude_module = None;

            // The sysroot's std and core crates provide their own preludes, even without an
            // `extern crate self as std` or `core` alias. Resolve their local prelude first:
            // falling back to core inside std would leave types such as Box unresolved.
            // Other crates prefer std's edition prelude, falling back to core when unavailable.
            // TODO: Parse crate-level `#![no_std]` and use it to select `core` prelude directly
            // and avoid exposing `std` as an automatic extern root for that crate.
            let prelude_paths = [
                matches!(
                    workspace_package.origin,
                    PackageOrigin::Sysroot(SysrootCrate::Std | SysrootCrate::Core)
                )
                .then(|| {
                    Path::crate_relative_standard_prelude(workspace_package.edition, interner)
                }),
                Some(Path::standard_prelude(
                    "std",
                    workspace_package.edition,
                    interner,
                )),
                Some(Path::standard_prelude(
                    "core",
                    workspace_package.edition,
                    interner,
                )),
            ];

            for prelude_path in prelude_paths.into_iter().flatten() {
                let root_module = ModuleRef::krate(state.crate_ref, state.root_module);
                prelude_module =
                    ScopeResolver::new(&env).import_module(root_module, &prelude_path)?;
                if prelude_module.is_some() {
                    break;
                }
            }

            let Some(prelude_module) = prelude_module else {
                continue;
            };

            let package_preludes = selected_preludes[package_slot]
                .as_mut()
                .expect("prelude slots should exist for every dirty package");
            package_preludes[crate_slot] = Some(prelude_module);
        }
    }

    // Apply the selected modules after lookup is done so future import resolution can consult the
    // prelude through `CrateResolutionEnv::prelude_module`.
    for (package_slot, package_states) in crates.iter_dirty_mut_enumerated() {
        let package_preludes = selected_preludes[package_slot]
            .as_ref()
            .expect("prelude slots should exist for every dirty package");
        for (crate_slot, state) in package_states.iter_mut().enumerate() {
            state.prelude = package_preludes[crate_slot];
        }
    }

    Ok(())
}

/// Resolves imports and item-position macros until every dirty crate scope stops changing.
///
/// Imports can depend on names introduced by other imports, and macro calls can depend on imports
/// that make the macro definition visible. This function therefore runs a small fixed-point loop:
/// resolve imports against the current crate builders, expand the macros that are now visible,
/// splice generated items back into the mutable crate builders, and refresh imports whenever those
/// generated items may have introduced new imports or exported names.
///
/// Source discovery happens inside the requesting macro batch. After loading, collect the real
/// module/include into the same scopes and continue calls appended since before that batch. Import
/// refresh waits until that queue drains, just as it does for directly generated declarations.
pub(crate) fn build_scopes(
    baseline: &DefMapReadTxn<'_>,
    parse: &mut ParseDb,
    item_tree: &mut ItemTreeDb,
    crates: &mut CrateBuildStore,
    interners: &mut PackageNameInterners,
    performance_preference: MacroExpansionPerformancePreference,
    source_loader: &mut impl MacroSourceFileLoader,
) -> anyhow::Result<()> {
    metric::EXPANSION_PASS_LIMIT.record_count(MAX_MACRO_EXPANSION_PASSES);

    // One call owns the complete fixed point. Loading files cannot reset the expansion budget or
    // discard compiled macros, import workers, or the scopes already produced by this build.
    let mut macro_runtime = MacroExpansionRuntime::new(performance_preference);
    let mut import_executor = ImportResolutionExecutor::new(performance_preference);
    let mut import_resolution_runs = 0;
    let mut expansion_passes = 0;
    let mut macro_source_file_resolutions = MacroSourceFileResolutions::default();

    'construction: loop {
        metric::ROUNDS.inc();
        let timer = metric::TIMING_RESOLVE_IMPORT_SCOPES.start_timer();
        resolve_import_scopes(
            baseline,
            crates,
            &mut import_executor,
            &mut import_resolution_runs,
        )?;
        timer.finish();
        let mut next_scan_cursors = None;
        // Direct additions are visible immediately; dependent imports and their reports need replay.
        let mut needs_import_refresh = false;

        // Macro expansion can introduce more macro calls visible through direct bindings. Keep
        // consuming that local queue before paying for another full import pass.
        loop {
            if expansion_passes >= MAX_MACRO_EXPANSION_PASSES {
                // Stop expanding but still freeze a coherent def-map. The final import refresh lets
                // names generated before the cap settle into module scopes.
                mark_retryable_macros_skipped_by_limit(crates);
                let timer = metric::TIMING_RESOLVE_IMPORT_SCOPES.start_timer();
                resolve_import_scopes(
                    baseline,
                    crates,
                    &mut import_executor,
                    &mut import_resolution_runs,
                )?;
                timer.finish();
                break 'construction;
            }

            expansion_passes += 1;
            metric::EXPANSION_PASSES.inc();

            // Prepare a whole batch against borrowed builders. Those borrows end before runtime
            // jobs execute or the source loader is allowed to grow Parse and ItemTree.
            let timer = metric::TIMING_COLLECT_EXPANSION_ATTEMPTS.start_timer();
            let mut expansion_attempts = {
                let env = BuildResolutionEnv::new(baseline, crates);
                // The first pass in a round visits all pending macro calls. Follow-up passes only
                // visit calls appended by the previous expansion, because older unresolved calls
                // need a fresh import snapshot before their answer can change.
                let scan = next_scan_cursors
                    .as_ref()
                    .map(MacroExpansionScan::NewCallsSince)
                    .unwrap_or(MacroExpansionScan::AllPending);
                collect_expansion_attempts(
                    &env,
                    crates,
                    scan,
                    &mut macro_runtime,
                    &macro_source_file_resolutions,
                )?
            };
            timer.finish();

            if expansion_attempts
                .iter()
                .any(|attempt| attempt.needs_expansion())
            {
                // The runtime owns the worker pool and creates it lazily, so projects without
                // expandable declarative macros do not pay its setup cost.
                expand_expansion_attempts(&mut macro_runtime, &mut expansion_attempts)?;
            }

            // Apply results in original attempt order. Capture the queue boundary first so both
            // directly generated calls and calls in newly loaded files remain in the next scan.
            let scan_cursors_before_apply = MacroExpansionCursors::capture(crates);
            let timer = metric::TIMING_APPLY_EXPANSION_ATTEMPTS.start_timer();
            let declarations_changed = if expansion_attempts.is_empty() {
                false
            } else {
                // Expanded text is parsed into regular item-tree data and appended to the owning
                // module. Direct bindings also enter its resolved scope, so simple chains like
                // `make_macro!(); generated_macro!();` work in one round.
                apply_expansion_attempts(
                    item_tree,
                    crates,
                    interners,
                    expansion_attempts,
                    &macro_source_file_resolutions,
                )?
            };
            timer.finish();

            needs_import_refresh |= declarations_changed;
            if declarations_changed {
                crates.censor_proc_macro_exports();
            }

            // All lookup and item borrows have ended before the loader can grow Parse/ItemTree.
            // Complete this batch before collecting real source into the requesting scopes.
            let requests = crates.macro_source_file_requests();
            if !requests.is_empty() {
                let resolutions = source_loader
                    .load_files(parse, item_tree, interners, &requests)
                    .context("load macro source files")?;
                anyhow::ensure!(
                    resolutions.len() == requests.len(),
                    "macro source loader must return one result per request",
                );
                for (request, resolution) in requests.into_iter().zip(resolutions) {
                    anyhow::ensure!(
                        matches!(
                            (&request, &resolution),
                            (_, MacroSourceFileResolution::Missing)
                                | (
                                    MacroSourceFileRequest::Module { .. },
                                    MacroSourceFileResolution::Module { .. }
                                )
                                | (
                                    MacroSourceFileRequest::Include { .. },
                                    MacroSourceFileResolution::Include { .. }
                                )
                        ),
                        "macro source loader returned the wrong file kind for {}",
                        request.description(),
                    );
                    macro_source_file_resolutions.insert(request, resolution);
                }
                crates.clear_macro_source_file_requests();
                crates.refresh_known_module_files(parse.packages(), item_tree);
                apply_pending_macro_source_files(
                    item_tree,
                    crates,
                    &macro_source_file_resolutions,
                )?;
                crates.censor_proc_macro_exports();

                // Even a missing result belongs to a batch that may have added other declarations.
                // Drain both its generated calls and the newly loaded calls before refreshing
                // imports, then require matching import reports before anything can freeze.
                needs_import_refresh = true;
                next_scan_cursors = Some(scan_cursors_before_apply);
                continue;
            }

            if declarations_changed {
                // Generated calls can be resolved with the same module scopes, but generated
                // imports cannot. Keep the cheap path going until no more direct expansion happens.
                next_scan_cursors = Some(scan_cursors_before_apply);
                continue;
            }

            if needs_import_refresh {
                // At least one expansion happened in this round. Re-run import resolution so
                // generated `use` items and newly exported names can participate in path lookup.
                break;
            }

            // No imports or macros changed the visible declarations. Module builders now own the
            // settled scopes and reports; only builtin impl synthesis remains before freezing.
            break 'construction;
        }
    }

    // Derives add impls without changing module bindings. Run them once after both ordinary
    // expansion and import resolution have settled, including the expansion-limit exit.
    let derives = BuiltinDeriveExpansion::collect(
        &BuildResolutionEnv::new(baseline, crates),
        crates,
        item_tree,
        interners,
    )
    .context("lower builtin derive impls")?;
    for derive in derives {
        derive.apply(crates);
    }
    Ok(())
}

/// Settle import-derived bindings without rebuilding every import group on every wave.
///
/// The worklist starts with every module that owns imports. Each wave rebuilds only the active
/// modules against one shared scope snapshot, installs all changed candidates together, and wakes
/// groups which read a changed scope. For a chain such as `root -> bridge -> source`, this lets
/// `bridge` gain a binding first and wakes `root` for the following wave.
///
/// When no changed scope has a reader, both the scopes and their latest unresolved-import reports
/// are ready to freeze.
fn resolve_import_scopes(
    baseline: &DefMapReadTxn<'_>,
    crates: &mut CrateBuildStore,
    import_executor: &mut ImportResolutionExecutor,
    import_resolution_runs: &mut usize,
) -> anyhow::Result<()> {
    metric::IMPORT_RESOLUTION_RUNS.inc();
    *import_resolution_runs += 1;
    let run_index = *import_resolution_runs;
    let mut pass_index = 0;
    let mut worklist = ImportWorklist::new(crates);

    while !worklist.active_modules().is_empty() {
        pass_index += 1;
        metric::IMPORT_RESOLUTION_PASSES.inc();

        // Finish the active wave before changing module scopes. If `root` imports from `bridge`
        // and `bridge` imports from `source`, `root` must not see `bridge`'s new binding until the
        // next wave; otherwise the result would depend on which worker happened to finish first.
        let updates = {
            let env = BuildResolutionEnv::new(baseline, crates);
            import_executor
                .apply_wave(&worklist, crates, &env)
                .context("while attempting to resolve import wave")?
        };
        let outcome = worklist.apply_updates(crates, updates);
        record_import_resolution_pass(
            run_index,
            pass_index,
            ImportResolutionPassMetrics {
                evaluated_packages: outcome.evaluated.packages,
                evaluated_crates: outcome.evaluated.crates,
                evaluated_modules: outcome.evaluated.modules,
                changed_packages: outcome.changed.packages,
                changed_crates: outcome.changed.crates,
                changed_modules: outcome.changed.modules,
                imports_evaluated: outcome.stats.imports_evaluated,
                glob_imports_evaluated: outcome.stats.glob_imports_evaluated,
                glob_bindings_emitted: outcome.stats.glob_bindings_emitted,
            },
        );

        if outcome.finished {
            break;
        }
    }

    Ok(())
}
