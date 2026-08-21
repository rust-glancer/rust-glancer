//! Resolves lowered Body IR while a build mutator has privileged package access.

use std::{
    collections::{BTreeSet, VecDeque},
    num::NonZeroUsize,
    sync::Mutex,
    time::{Duration, Instant},
};

use anyhow::Context as _;
use rayon::prelude::*;
use rg_def_map::{DefMapReadTxn, PackageSlot};
use rg_ir_model::{CrateId, CrateRef};
use rg_semantic_ir::{ItemLookupQueryCache, SemanticIrReadTxn};
use rg_std::Shrink;
use rg_text::{NameInterner, PackageNameInterners};
use rg_ty::TraitSelectionDeclarationCache;

use crate::{CrateBodies, PackageBodies};

use super::{local_thread_pool, lower::LoweredPackageBodies, state::CrateBodyBuildState};

// Package resolution runs in parallel, so report only packages large enough to stand out from
// normal scheduling variance.
const SLOW_PACKAGE_RESOLUTION: Duration = Duration::from_secs(2);

/// Resolve all materialized packages against one semantic snapshot.
///
/// Each crate still needs its own trait-selection session because impl visibility and solver
/// answers depend on the use-site crate. Canonical crate declaration shapes do not, so package
/// workers share one build-scoped declaration cache.
pub(super) fn resolve_packages(
    packages: Vec<LoweredPackageBodies>,
    parse: &rg_parse::ParseDb,
    interners: &mut PackageNameInterners,
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    worker_limit: Option<NonZeroUsize>,
) -> anyhow::Result<Vec<PackageBodies>> {
    let profile_context = rg_profile::ProfileThreadContext::capture();
    let declarations = TraitSelectionDeclarationCache::new();
    let item_lookup_cache = ItemLookupQueryCache::new();
    let thread_pool = local_thread_pool("rg-body-resolve", worker_limit)?;
    let resolved = thread_pool
        .install(|| {
            packages
                .into_par_iter()
                .zip(parse.packages().par_iter())
                .zip(interners.packages_mut().par_iter_mut())
                .enumerate()
                .map(|(package_idx, ((package, parse_package), interner))| {
                    let _profile_guard = profile_context.enter();
                    resolve_package(
                        PackageSlot(package_idx),
                        parse_package,
                        package,
                        interner,
                        def_map,
                        semantic_ir,
                        &declarations,
                        &item_lookup_cache,
                    )
                })
                .collect::<anyhow::Result<Vec<_>>>()
        })
        .context("while attempting to resolve body IR packages")?;
    record_lookup_cache_stats(&item_lookup_cache);
    Ok(resolved)
}

/// Resolve a sparse set of rebuilt packages while preserving package and crate identities.
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
    priority_packages: Option<&(dyn Fn() -> Vec<PackageSlot> + Sync)>,
    publish_priority: &(dyn Fn(PackageSlot, PackageBodies) + Sync),
    worker_limit: Option<NonZeroUsize>,
) -> anyhow::Result<Vec<(PackageSlot, PackageBodies)>> {
    let profile_context = rg_profile::ProfileThreadContext::capture();
    let declarations = TraitSelectionDeclarationCache::new();
    let item_lookup_cache = ItemLookupQueryCache::new();
    // Selected rebuilds are sparse, but resolution may discover nested bodies and lower them,
    // which needs mutable access to the matching package name interner. The rebuilder normalizes
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
    let Some(priority_packages) = priority_packages else {
        let resolved = thread_pool
            .install(|| {
                jobs.into_par_iter()
                    .map(|(package_slot, parse_package, package, interner)| {
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
                        )?;
                        Ok((package_slot, package))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()
            })
            .context("while attempting to resolve selected body IR packages")?;
        record_lookup_cache_stats(&item_lookup_cache);
        return Ok(resolved);
    };

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
                    let priorities = priority_packages().into_iter().collect::<BTreeSet<_>>();
                    ResolvedPackage::publish_resolved_priorities(
                        &resolved,
                        &priorities,
                        publish_priority,
                    );

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

                    let result = (|| {
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
                        )?;
                        Ok(ResolvedPackage {
                            package: package_slot,
                            bodies: package,
                            priority_published: false,
                        })
                    })();
                    resolved
                        .lock()
                        .expect("Body IR package resolution results should not be poisoned")
                        .push(result);
                }
            });
        }
    });

    // Capture a priority update that raced with the last worker returning. If it arrived any later,
    // the complete result is already about to be published through the ordinary final path.
    let priorities = priority_packages().into_iter().collect::<BTreeSet<_>>();
    ResolvedPackage::publish_resolved_priorities(&resolved, &priorities, publish_priority);
    let mut resolved = resolved
        .into_inner()
        .expect("Body IR package resolution results should not be poisoned")
        .into_iter()
        .collect::<anyhow::Result<Vec<_>>>()?
        .into_iter()
        .map(|resolved| (resolved.package, resolved.bodies))
        .collect::<Vec<_>>();
    resolved.sort_by_key(|(package, _)| package.0);

    record_lookup_cache_stats(&item_lookup_cache);
    Ok(resolved)
}

fn record_lookup_cache_stats(cache: &ItemLookupQueryCache) {
    let stats = cache.stats();
    crate::profile::metric::DEPENDENCY_CACHE_CONSTRUCTIONS
        .add(stats.dependency_cache_constructions as u64);
    crate::profile::metric::DEPENDENCY_CACHE_REUSES.add(stats.dependency_cache_reuses as u64);
    crate::profile::metric::DEPENDENCY_RESULT_HITS.add(stats.dependency_result_hits as u64);
    crate::profile::metric::DEPENDENCY_RESULT_MISSES.add(stats.dependency_result_misses as u64);
}

#[derive(Debug)]
struct ResolvedPackage {
    package: PackageSlot,
    bodies: PackageBodies,
    priority_published: bool,
}

impl ResolvedPackage {
    /// Publish compact copies for packages that became editor priorities after resolving.
    fn publish_resolved_priorities(
        resolved: &Mutex<Vec<anyhow::Result<Self>>>,
        priorities: &BTreeSet<PackageSlot>,
        publish_priority: &(dyn Fn(PackageSlot, PackageBodies) + Sync),
    ) {
        let publications = {
            let mut resolved = resolved
                .lock()
                .expect("Body IR package resolution results should not be poisoned");
            resolved
                .iter_mut()
                .filter_map(|resolved| resolved.as_mut().ok())
                .filter(|resolved| {
                    !resolved.priority_published && priorities.contains(&resolved.package)
                })
                .map(|resolved| {
                    resolved.priority_published = true;
                    let mut publishable = resolved.bodies.clone();
                    Shrink::shrink_to_fit(&mut publishable);
                    (resolved.package, publishable)
                })
                .collect::<Vec<_>>()
        };

        for (package, bodies) in publications {
            publish_priority(package, bodies);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_package(
    package_slot: PackageSlot,
    parse_package: &rg_parse::Package,
    package: LoweredPackageBodies,
    interner: &mut NameInterner,
    def_map_txn: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    declarations: &TraitSelectionDeclarationCache,
    item_lookup_cache: &ItemLookupQueryCache,
) -> anyhow::Result<PackageBodies> {
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
        .enumerate()
        .map(|(crate_idx, crate_bodies)| {
            let coverage = crate_bodies.coverage();
            if !coverage.is_materialized() {
                return Ok(CrateBodies::empty(coverage));
            }

            let crate_ref = CrateRef {
                package: package_slot,
                crate_id: CrateId(crate_idx),
            };

            CrateBodyBuildState::new(crate_ref, parse_package, crate_bodies, interner).resolve(
                def_map_txn,
                semantic_ir,
                declarations,
                item_lookup_cache,
            )
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let elapsed = started.elapsed();
    if elapsed >= SLOW_PACKAGE_RESOLUTION {
        tracing::debug!(
            elapsed_ms = elapsed.as_millis(),
            crate_count,
            body_count = crates
                .iter()
                .map(|crate_bodies| crate_bodies.bodies().len())
                .sum::<usize>(),
            "slow Body IR package resolution"
        );
    }

    Ok(PackageBodies::new(crates))
}
