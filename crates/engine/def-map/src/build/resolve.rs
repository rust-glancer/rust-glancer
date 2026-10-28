//! Builds module scopes by alternating import resolution and item macro expansion.
//!
//! Imports can expose a macro, and that macro can emit more imports or declarations. The outer
//! loop collects import answers, then expands a bounded burst of calls made available by direct
//! declarations. Real files discovered by expansion are loaded before that burst continues. A burst
//! yields to imports and retries even if another chain keeps producing calls.
//!
//! One crate owns this fixed point; external crates are already frozen. Its scope buffers move
//! between the builder and the import collector throughout construction.
//! Macro expansion reads and extends the builder's scopes; import resolution takes those buffers,
//! updates them, and returns them before expansion resumes. Glob links, macro runtime caches,
//! and the scan cursor survive these handoffs. Recursion is limited independently per invocation.

use std::{mem, sync::Arc};

use anyhow::Context as _;
use rg_ir_model::{ModuleId, ModuleRef, Path};
use rg_item_tree::ItemTreeDb;
use rg_macro_runtime::MacroExpansionRuntime;
use rg_parse::ParseDb;
use rg_std::UniqueVec;
use rg_text::{NameInterner, PackageNameInterners};
use rg_workspace::{PackageOrigin, SysrootCrate};

use super::{
    MacroSourceFileResolutions,
    crates::{CrateBuilder, KnownModuleFiles},
    imports::ImportCollector,
    macros::{
        BuiltinDeriveExpansion, MAX_MACRO_EXPANSION_DEPTH, MacroExpansionScan,
        PendingMacroExpansionLimitReport, apply_expansion_attempts,
        apply_pending_macro_source_files, collect_expansion_attempts, expand_expansion_attempts,
    },
    output::CompletedCrateStore,
    resolution_env::BuildResolutionEnv,
};
use crate::{
    DefMapReadTxn, MacroSourceFileLoader, MacroSourceFileRequest, MacroSourceFileResolution,
    NameResolver,
    profile::{metric, record_import_resolution_run},
};

// Let local chains advance cheaply, but give generated imports and waiting calls a turn after
// this many passes. This is a scheduling boundary; it never discards remaining work.
const MAX_EXPANSION_BATCH_PASSES: usize = 32;

/// Dependencies are frozen before this lookup, so their edition-prelude paths already have
/// their final module structure. `std` and `core` first look for their own local prelude.
pub(crate) fn select_prelude(
    env: &BuildResolutionEnv<'_>,
    state: &CrateBuilder,
    origin: &PackageOrigin,
    interner: &mut NameInterner,
) -> anyhow::Result<Option<ModuleRef>> {
    // TODO: Parse `#![no_std]` to select core directly and omit the automatic std root.
    let prelude_paths = [
        matches!(
            origin,
            PackageOrigin::Sysroot(SysrootCrate::Std | SysrootCrate::Core)
        )
        .then(|| Path::crate_relative_standard_prelude(state.edition, interner)),
        Some(Path::standard_prelude("std", state.edition, interner)),
        Some(Path::standard_prelude("core", state.edition, interner)),
    ];
    let root = ModuleRef::krate(state.crate_ref, state.root_module);
    for path in prelude_paths.into_iter().flatten() {
        if let Some(prelude) = NameResolver::new(env).import_module(root, &path)? {
            return Ok(Some(prelude));
        }
    }
    Ok(None)
}

/// Settles one crate by alternating import collection and item-position macro expansion.
///
/// Imports can depend on names introduced by other imports, and macro calls can depend on imports
/// that make the macro definition visible. This function therefore runs a small fixed-point loop:
/// extend the retained scopes with import answers, expand the macros that are now visible,
/// splice generated items back into the active builder, and refresh imports whenever those
/// generated items may have introduced new imports or exported names.
///
/// Source discovery completes inside the requesting pass. After loading, collect the real
/// module/include before continuing its generated calls. Each bounded burst then yields to imports
/// and retries, even if some chains keep generating more work.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_crate(
    baseline: &DefMapReadTxn<'_>,
    completed: &CompletedCrateStore,
    parse: &mut ParseDb,
    item_tree: &mut ItemTreeDb,
    state: &mut CrateBuilder,
    interners: &mut PackageNameInterners,
    macro_runtime: &mut MacroExpansionRuntime,
    macro_source_file_resolutions: &mut MacroSourceFileResolutions,
    remaining_report_groups: &mut usize,
    source_loader: &mut impl MacroSourceFileLoader,
) -> anyhow::Result<()> {
    metric::MACRO_RECURSION_LIMIT.record_count(MAX_MACRO_EXPANSION_DEPTH);

    // Import scheduling belongs to this crate; recursion belongs to each invocation's ancestry.
    // The caller shares the macro runtime and source answers across crates.
    let mut import_collector = ImportCollector::default();
    let mut import_resolution_runs = 0;

    'construction: loop {
        metric::ROUNDS.inc();
        let timer = metric::TIMING_RESOLVE_IMPORT_SCOPES.start_timer();
        collect_imports(
            baseline,
            completed,
            state,
            &mut import_collector,
            &mut import_resolution_runs,
        )?;
        timer.finish();
        let mut next_scan_start = None;
        // Direct additions are visible immediately; dependent imports and their reports need refreshing.
        let mut needs_import_refresh = false;

        // Advance a bounded burst of direct expansions before refreshing imports. A recursive
        // chain cannot keep an independent call waiting for a generated `use` indefinitely.
        for _ in 0..MAX_EXPANSION_BATCH_PASSES {
            metric::EXPANSION_PASSES.inc();

            // Prepare a batch against the active builder. Its borrows end before runtime
            // jobs execute or the source loader is allowed to grow Parse and ItemTree.
            let timer = metric::TIMING_COLLECT_EXPANSION_ATTEMPTS.start_timer();
            let mut expansion_attempts = {
                let env = BuildResolutionEnv::new(baseline, completed, state);
                // The first pass in a round visits all pending macro calls. Follow-up passes only
                // visit calls appended by the previous expansion, because older unresolved calls
                // need another import refresh before their answer can change.
                let scan = next_scan_start
                    .map(MacroExpansionScan::NewCallsSince)
                    .unwrap_or(MacroExpansionScan::AllPending);
                collect_expansion_attempts(
                    &env,
                    state,
                    scan,
                    macro_runtime,
                    macro_source_file_resolutions,
                )?
            };
            timer.finish();

            if expansion_attempts
                .iter()
                .any(|attempt| attempt.needs_expansion())
            {
                // The runtime owns the worker pool and creates it lazily, so projects without
                // expandable declarative macros do not pay its setup cost.
                expand_expansion_attempts(macro_runtime, &mut expansion_attempts)?;
            }

            // Apply results in original attempt order. Capture the queue boundary first so both
            // directly generated calls and calls in newly loaded files remain in the next scan.
            let call_count_before_apply = state.macro_directives.len();
            let timer = metric::TIMING_APPLY_EXPANSION_ATTEMPTS.start_timer();
            let declarations_changed = if expansion_attempts.is_empty() {
                false
            } else {
                // Expanded text is parsed into regular item-tree data and appended to the owning
                // module. Direct bindings enter its scope immediately, so simple chains like
                // `make_macro!(); generated_macro!();` work in one round.
                let package = item_tree
                    .package(state.crate_ref.package.0)
                    .expect("active crate has an item-tree package");
                let interner = interners
                    .package_mut(state.crate_ref.package.0)
                    .expect("active crate has a name interner");
                apply_expansion_attempts(
                    package,
                    state,
                    interner,
                    expansion_attempts,
                    macro_source_file_resolutions,
                )
            };
            timer.finish();

            needs_import_refresh |= declarations_changed;
            if declarations_changed {
                state.censor_proc_macro_exports();
            }

            // All lookup and item borrows have ended before the loader can grow Parse/ItemTree.
            // Complete this batch before collecting real source into the requesting scopes.
            let requests = mem::take(&mut state.macro_source_file_requests)
                .into_iter()
                .collect::<UniqueVec<_>>()
                .into_vec();
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
                // Reborrow the package after loading: its ItemTree may have grown, and no
                // borrowed input from the preceding expansion batch survives this boundary.
                let package = item_tree
                    .package(state.crate_ref.package.0)
                    .expect("active crate has an item-tree package");
                state.known_module_files = Arc::new(KnownModuleFiles::from_package(
                    parse
                        .package(state.crate_ref.package.0)
                        .expect("active package remains parsed"),
                    package,
                ));
                apply_pending_macro_source_files(package, state, macro_source_file_resolutions)?;
                state.censor_proc_macro_exports();

                // Even a missing result belongs to a batch that may have added declarations.
                // Newly loaded calls can run in the rest of this burst; imports refresh when the
                // burst drains or yields, before unresolved calls are retried.
                needs_import_refresh = true;
                next_scan_start = Some(call_count_before_apply);
                continue;
            }

            if declarations_changed {
                // Direct calls can continue within this burst. Generated imports participate
                // when the burst drains or reaches its scheduling boundary.
                next_scan_start = Some(call_count_before_apply);
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

    // Only recursion-limited invocations belong in the report. Ordinary lookup misses remain
    // unresolved, and sibling calls have had the opportunity to finish before we get here.
    state.macro_expansion_limit =
        PendingMacroExpansionLimitReport::collect(&state.macro_directives, remaining_report_groups);

    // Derives add impls without changing module bindings. Run them once after imports and
    // ordinary expansion have settled.
    let package = item_tree
        .package(state.crate_ref.package.0)
        .expect("active crate has an item-tree package");
    let interner = interners
        .package_mut(state.crate_ref.package.0)
        .expect("active crate has a name interner");
    let derives = BuiltinDeriveExpansion::collect(
        &BuildResolutionEnv::new(baseline, completed, state),
        state,
        package,
        interner,
    )
    .context("lower builtin derive impls")?;
    for derive in derives {
        derive.apply(state);
    }
    Ok(())
}

/// Lend scope buffers to the import collector, then return them for macro lookup.
///
/// Only modules with imports need to move: other modules can supply names through the environment
/// without receiving imported bindings. While these buffers are out, the collector overlays them
/// on environment reads. Return both scopes and reports before the next macro batch can use them.
fn collect_imports(
    baseline: &DefMapReadTxn<'_>,
    completed: &CompletedCrateStore,
    state: &mut CrateBuilder,
    import_collector: &mut ImportCollector,
    import_resolution_runs: &mut usize,
) -> anyhow::Result<()> {
    metric::IMPORT_RESOLUTION_RUNS.inc();
    *import_resolution_runs += 1;
    state.censor_proc_macro_exports();
    let mut scopes = Vec::new();
    for index in 0..state.def_map_builder.module_count() {
        let module = ModuleId(index);
        if state
            .def_map_builder
            .module(module)
            .is_some_and(|data| !data.imports.is_empty())
        {
            scopes.push((
                ModuleRef::krate(state.crate_ref, module),
                state.def_map_builder.take_module_scope(module),
            ));
        }
    }
    let result =
        import_collector.collect(&BuildResolutionEnv::new(baseline, completed, state), scopes)?;
    record_import_resolution_run(state.crate_ref, *import_resolution_runs, result.metrics);
    for module in result.modules {
        state.def_map_builder.apply_module_imports(
            module.module.module,
            module.scope,
            module.unresolved,
        );
    }
    Ok(())
}
