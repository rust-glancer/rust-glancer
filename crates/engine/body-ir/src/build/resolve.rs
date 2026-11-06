//! Resolves independent crate payloads against immutable saved declarations.

use std::time::{Duration, Instant};

use anyhow::Context as _;
use rayon::prelude::*;
use rg_def_map::DefMapReadTxn;
use rg_ir_model::{CrateRef, PackageSlot};
use rg_semantic_ir::{ItemLookupQueryCache, SemanticIrReadTxn};
use rg_text::{NameInterner, PackageNameInterners};

use super::{local_thread_pool, lower::LoweredPackageBodies, state::CrateBodyBuildState};
use crate::CrateBodies;

// Package resolution runs in parallel, so report only packages large enough to stand out from
// normal scheduling variance.
const SLOW_PACKAGE_RESOLUTION: Duration = Duration::from_secs(2);

/// Resolve selected packages while preserving package and crate identities.
///
/// Before starting Rayon jobs, give each package mutable access to its own name interner. No two
/// workers can then touch the same interner, so package resolution needs no extra synchronization.
/// The jobs borrow immutable crate declarations. Each body owns its inference state and cache of
/// lowered declarations, while its crate determines which impls and language items are visible.
#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_selected_packages(
    packages: Vec<(PackageSlot, LoweredPackageBodies)>,
    parse: &rg_parse::ParseDb,
    interners: &mut PackageNameInterners,
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    cancellation: &rg_std::CancellationToken,
) -> anyhow::Result<Vec<(CrateRef, CrateBodies)>> {
    let profile_context = rg_profile::ProfileThreadContext::capture();
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

    let thread_pool = local_thread_pool("rg-body-resolve")?;
    let results = thread_pool.install(|| {
        jobs.into_par_iter()
            .map(|(package_slot, parse_package, package, interner)| {
                let _profile_guard = profile_context.enter();
                resolve_package(
                    package_slot,
                    parse_package,
                    package,
                    interner,
                    def_map,
                    semantic_ir,
                    &item_lookup_cache,
                    cancellation,
                )
            })
            .collect::<Vec<_>>()
    });

    // Every worker has joined. Keep source failures visible if cancellation also reached a
    // sibling; callers receive no partial products when any package failed.
    let mut products = Vec::new();
    let mut cancelled = None;
    for result in results {
        match result {
            Ok(mut package) => products.append(&mut package),
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
    Ok(products)
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
            .resolve(def_map_txn, semantic_ir, item_lookup_cache)
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
