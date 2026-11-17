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
use rg_macro_runtime::MacroExpansionRuntime;
use rg_std::UniqueVec;
use rg_workspace::{PackageOrigin, SysrootCrate};

use super::{
    MacroSourceFileResolutions,
    crates::CrateBuilder,
    imports::ImportCollector,
    macros::{
        BuiltinDeriveExpansion, MAX_MACRO_EXPANSION_DEPTH, MacroExpansionScan,
        PendingMacroExpansionLimitReport, apply_expansion_attempts,
        apply_pending_macro_source_files, collect_expansion_attempts, expand_expansion_attempts,
    },
    resolution_env::BuildResolutionEnv,
    target::TargetCrateSources,
};
use crate::{
    CrateResolutionEnv, MacroDefinitionEnv, NameResolver,
    profile::{metric, record_import_resolution_run},
};

// Let local chains advance cheaply, but give generated imports and waiting calls a turn after
// this many passes. This is a scheduling boundary; it never discards remaining work.
const MAX_EXPANSION_BATCH_PASSES: usize = 32;
const MAX_REPORTED_MACRO_GROUPS: usize = 64;

/// Dependencies are frozen before this lookup, so their edition-prelude paths already have
/// their final module structure. `std` and `core` first look for their own local prelude.
pub(crate) fn select_prelude(
    env: &impl CrateResolutionEnv,
    state: &CrateBuilder,
    origin: &PackageOrigin,
) -> anyhow::Result<Option<ModuleRef>> {
    // TODO: Parse `#![no_std]` to select core directly and omit the automatic std root.
    let prelude_paths = [
        matches!(
            origin,
            PackageOrigin::Sysroot(SysrootCrate::Std | SysrootCrate::Core)
        )
        .then(|| Path::crate_relative_standard_prelude(state.edition)),
        Some(Path::standard_prelude("std", state.edition)),
        Some(Path::standard_prelude("core", state.edition)),
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
pub(super) fn resolve_crate<E>(
    dependencies: &E,
    sources: &mut TargetCrateSources<'_>,
    state: &mut CrateBuilder,
) -> anyhow::Result<()>
where
    E: CrateResolutionEnv + MacroDefinitionEnv,
{
    metric::MACRO_RECURSION_LIMIT.record_count(MAX_MACRO_EXPANSION_DEPTH);

    // Import scheduling belongs to this crate; recursion belongs to each invocation's ancestry.
    let mut macro_runtime = MacroExpansionRuntime::new();
    let mut macro_source_file_resolutions = MacroSourceFileResolutions::new();
    let mut import_collector = ImportCollector::default();
    let mut import_resolution_runs = 0;

    'construction: loop {
        sources.check_cancelled()?;
        metric::ROUNDS.inc();
        let timer = metric::TIMING_RESOLVE_IMPORT_SCOPES.start_timer();
        collect_imports(
            dependencies,
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
            sources.check_cancelled()?;
            metric::EXPANSION_PASSES.inc();

            // Prepare a batch against the active builder. Its borrows end before runtime
            // jobs execute or discovery can add item trees.
            let timer = metric::TIMING_COLLECT_EXPANSION_ATTEMPTS.start_timer();
            let package = sources.trees();
            let mut expansion_attempts = {
                let env = BuildResolutionEnv::new(dependencies, state);
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
            let call_count_before_apply = state.macro_directives.len();
            let timer = metric::TIMING_APPLY_EXPANSION_ATTEMPTS.start_timer();
            let declarations_changed = if expansion_attempts.is_empty() {
                false
            } else {
                // Expanded text is parsed into regular item-tree data and appended to the owning
                // module. Direct bindings enter its scope immediately, so simple chains like
                // `make_macro!(); generated_macro!();` work in one round.
                apply_expansion_attempts(
                    package,
                    state,
                    expansion_attempts,
                    &macro_source_file_resolutions,
                )
            };
            timer.finish();

            needs_import_refresh |= declarations_changed;
            if declarations_changed {
                state.censor_proc_macro_exports();
            }

            // All lookup and item borrows have ended before discovery can add item trees.
            // Complete this batch before collecting real source into the requesting scopes.
            let requests = mem::take(&mut state.macro_source_file_requests)
                .into_iter()
                .collect::<UniqueVec<_>>()
                .into_vec();
            if !requests.is_empty() {
                for request in requests {
                    let resolution = sources
                        .load_file(&request)
                        .context("load macro source file")?;
                    macro_source_file_resolutions.insert(request, resolution);
                }
                // Reborrow the package after loading: its ItemTree may have grown, and no
                // borrowed input from the preceding expansion batch survives this boundary.
                state.known_module_files = Arc::new(sources.known_module_files());
                apply_pending_macro_source_files(
                    sources.trees(),
                    state,
                    &macro_source_file_resolutions,
                )?;
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
    let mut remaining_report_groups = MAX_REPORTED_MACRO_GROUPS;
    state.macro_expansion_limit = PendingMacroExpansionLimitReport::collect(
        &state.macro_directives,
        &mut remaining_report_groups,
    );

    // Derives add impls without changing module bindings. Run them once after imports and
    // ordinary expansion have settled.
    let package = sources.trees();
    let derives = BuiltinDeriveExpansion::collect(
        &BuildResolutionEnv::new(dependencies, state),
        state,
        package,
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
fn collect_imports<E: CrateResolutionEnv>(
    dependencies: &E,
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
    let result = import_collector.collect(&BuildResolutionEnv::new(dependencies, state), scopes)?;
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
