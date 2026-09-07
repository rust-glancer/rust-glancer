//! Resolves independent crate payloads against immutable saved declarations.

use std::{
    collections::{BTreeSet, VecDeque},
    num::NonZeroUsize,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::Context as _;
use rg_def_map::{DefMapReadTxn, PackageSlot};
use rg_ir_model::CrateRef;
use rg_semantic_ir::{ItemLookupQueryCache, SemanticIrReadTxn};
use rg_text::{NameInterner, PackageNameInterners};
use rg_ty::TraitSelectionDeclarationCache;

use crate::CrateBodies;

use super::{
    BodyIrBuildProgress, BodyIrBuildStage, local_thread_pool, lower::LoweredPackageBodies,
    state::CrateBodyBuildState,
};

// Package resolution runs in parallel, so report only packages large enough to stand out from
// normal scheduling variance.
const SLOW_PACKAGE_RESOLUTION: Duration = Duration::from_secs(2);

/// Resolve selected packages while preserving package and crate identities.
///
/// Before starting Rayon jobs, give each package mutable access to its own name interner. No two
/// workers can then touch the same interner, so package resolution needs no extra synchronization.
/// The jobs also share canonical crate declaration lowering, while keeping their visibility and
/// solver state inside the corresponding crate session.
#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_selected_packages(
    packages: Vec<(PackageSlot, LoweredPackageBodies)>,
    parse: &rg_parse::ParseDb,
    interners: &mut PackageNameInterners,
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    priority_packages: &(dyn Fn() -> Vec<PackageSlot> + Sync),
    publish: &(dyn Fn(Vec<(CrateRef, CrateBodies)>) + Sync),
    worker_limit: Option<NonZeroUsize>,
    report_progress: Option<&(dyn Fn(BodyIrBuildProgress) + Sync)>,
    cancellation: &rg_std::CancellationToken,
) -> anyhow::Result<()> {
    let profile_context = rg_profile::ProfileThreadContext::capture();
    let declarations = TraitSelectionDeclarationCache::new();
    let item_lookup_cache = ItemLookupQueryCache::new();
    // Selected builds can be sparse, but resolution may discover nested bodies and lower them,
    // which needs mutable access to the matching package name interner. The builder normalizes
    // package slots, so walking the interner slice left-to-right lets us prepare disjoint jobs that
    // Rayon can resolve in parallel without hiding any aliasing behind helper abstractions.
    let parse_packages = parse.packages();
    let mut remaining_interners = interners.packages_mut();
    let mut next_package_idx = 0;
    let mut jobs = Vec::with_capacity(packages.len());

    for (package_slot, package) in packages {
        anyhow::ensure!(
            package_slot.0 >= next_package_idx,
            "selected body IR packages must be sorted and unique, but package {} appeared after {}",
            package_slot.0,
            next_package_idx.saturating_sub(1),
        );
        let parse_package = parse_packages.get(package_slot.0).with_context(|| {
            format!("while attempting to fetch parse package {}", package_slot.0)
        })?;
        let skip = package_slot.0 - next_package_idx;
        if skip >= remaining_interners.len() {
            anyhow::bail!(
                "while attempting to fetch name interner for package {}",
                package_slot.0,
            );
        }
        let (_, tail) = remaining_interners.split_at_mut(skip);
        let (interner, rest) = tail
            .split_first_mut()
            .expect("interner slice should contain selected package after bounds check");

        jobs.push((package_slot, parse_package, package, interner));
        remaining_interners = rest;
        next_package_idx = package_slot.0 + 1;
    }

    let thread_pool = local_thread_pool("rg-body-resolve", worker_limit)?;
    let total_packages = jobs.len();
    let completed_packages = AtomicUsize::new(0);
    // Rayon normally commits the entire indexed iterator to its work-stealing queues up front.
    // Keep package jobs in this small shared queue instead so a didOpen arriving during resolution
    // can move its package ahead of work that has not started yet. Workers hold the lock only while
    // selecting a package; resolution itself remains fully parallel.
    let job_count = jobs.len();
    let jobs = Mutex::new(VecDeque::from(jobs));
    let resolved = Mutex::new(Vec::with_capacity(job_count));
    let worker_count = thread_pool.current_num_threads().min(job_count);
    thread_pool.scope(|scope| {
        for _ in 0..worker_count {
            scope.spawn(|_| {
                loop {
                    if cancellation.is_cancelled() {
                        break;
                    }
                    let priorities = priority_packages().into_iter().collect::<BTreeSet<_>>();

                    let job = {
                        let mut jobs = jobs
                            .lock()
                            .expect("Body IR package resolution queue should not be poisoned");
                        let job_idx = jobs
                            .iter()
                            .position(|(package, _, _, _)| priorities.contains(package))
                            .unwrap_or(0);
                        jobs.remove(job_idx)
                    };
                    let Some((package_slot, parse_package, package, interner)) = job else {
                        break;
                    };

                    let result: anyhow::Result<()> = (|| {
                        let _profile_guard = profile_context.enter();
                        let package = resolve_package(
                            package_slot,
                            parse_package,
                            package,
                            interner,
                            def_map,
                            semantic_ir,
                            &declarations,
                            &item_lookup_cache,
                            cancellation,
                        )?;
                        // Transfer each finished payload once. A later cancellation can discard
                        // unfinished work without retracting products already delivered.
                        rg_std::check_cancel!(cancellation, "deliver resolved body products");
                        publish(package);
                        Ok(())
                    })();
                    if result.is_ok()
                        && let Some(report_progress) = report_progress
                    {
                        let completed_packages =
                            completed_packages.fetch_add(1, Ordering::Relaxed) + 1;
                        report_progress(BodyIrBuildProgress::new(
                            BodyIrBuildStage::Resolving,
                            completed_packages,
                            total_packages,
                        ));
                    }
                    resolved
                        .lock()
                        .expect("Body IR package resolution results should not be poisoned")
                        .push(result);
                }
            });
        }
    });

    // The scope has joined every started job. Prefer source failures over sibling cancellation.
    // Completed products have already been transferred; checking these statuses does not undo them.
    let results = resolved
        .into_inner()
        .expect("body resolution results should not be poisoned");
    let mut cancelled = None;
    for result in results {
        match result {
            Ok(()) => {}
            Err(error) if error.chain().any(|cause| cause.is::<rg_std::Cancelled>()) => {
                cancelled.get_or_insert(error);
            }
            Err(error) => return Err(error),
        }
    }
    if let Some(error) = cancelled {
        return Err(error);
    }
    rg_std::check_cancel!(cancellation, "join body resolution workers");
    record_lookup_cache_stats(&item_lookup_cache);
    Ok(())
}

fn record_lookup_cache_stats(cache: &ItemLookupQueryCache) {
    let stats = cache.stats();
    crate::profile::metric::DEPENDENCY_CACHE_CONSTRUCTIONS
        .add(stats.dependency_cache_constructions as u64);
    crate::profile::metric::DEPENDENCY_CACHE_REUSES.add(stats.dependency_cache_reuses as u64);
    crate::profile::metric::DEPENDENCY_RESULT_HITS.add(stats.dependency_result_hits as u64);
    crate::profile::metric::DEPENDENCY_RESULT_MISSES.add(stats.dependency_result_misses as u64);
}

#[allow(clippy::too_many_arguments)]
#[rg_std::cancelable("resolve body package", token = cancellation)]
fn resolve_package(
    package_slot: PackageSlot,
    parse_package: &rg_parse::Package,
    package: LoweredPackageBodies,
    interner: &mut NameInterner,
    def_map_txn: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    declarations: &TraitSelectionDeclarationCache,
    item_lookup_cache: &ItemLookupQueryCache,
    cancellation: &rg_std::CancellationToken,
) -> anyhow::Result<Vec<(CrateRef, CrateBodies)>> {
    let crate_count = package.len();
    let span = tracing::debug_span!(
        "body_ir_package_resolution",
        rg.package = parse_package.package_name(),
        rg.package_slot = package_slot.0,
    );
    let _entered = span.enter();
    let started = Instant::now();

    let crates = package
        .into_iter()
        .map(|(crate_id, crate_bodies)| {
            rg_std::check_cancel!(cancellation, "resolve body crate");
            let coverage = crate_bodies.coverage();
            if !coverage.is_materialized() {
                return Ok((
                    CrateRef {
                        package: package_slot,
                        crate_id,
                    },
                    CrateBodies::empty(coverage),
                ));
            }

            let crate_ref = CrateRef {
                package: package_slot,
                crate_id,
            };

            CrateBodyBuildState::new(
                crate_ref,
                parse_package,
                crate_bodies,
                interner,
                cancellation.clone(),
            )
            .resolve(def_map_txn, semantic_ir, declarations, item_lookup_cache)
            .map(|bodies| (crate_ref, bodies))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let elapsed = started.elapsed();
    if elapsed >= SLOW_PACKAGE_RESOLUTION {
        tracing::debug!(
            elapsed_ms = elapsed.as_millis(),
            crate_count,
            body_count = crates
                .iter()
                .map(|(_, crate_bodies)| crate_bodies.bodies().len())
                .sum::<usize>(),
            "slow Body IR package resolution"
        );
    }

    Ok(crates)
}
