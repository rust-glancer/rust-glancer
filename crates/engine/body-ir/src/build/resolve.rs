//! Resolves independent crate payloads against immutable saved declarations.

use std::time::{Duration, Instant};

use anyhow::Context as _;
use rayon::prelude::*;
use rg_def_map::DefMapReadTxn;
use rg_ir_model::{CrateRef, PackageSlot};
use rg_semantic_ir::{ItemLookupQueryCache, SemanticIrReadTxn};

use super::{local_thread_pool, lower::LoweredPackageBodies, state::CrateBodyBuildState};
use crate::CrateBodies;

// Package resolution runs in parallel, so report only packages large enough to stand out from
// normal scheduling variance.
const SLOW_PACKAGE_RESOLUTION: Duration = Duration::from_secs(2);

/// Resolve selected packages while preserving package and crate identities.
///
/// The jobs borrow immutable crate declarations. Each body owns its inference state and cache of
/// lowered declarations, while its crate determines which impls and language items are visible.
pub(super) fn resolve_selected_packages(
    packages: Vec<(PackageSlot, LoweredPackageBodies)>,
    parse: &rg_parse::ParseDb,
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    cancellation: &rg_std::CancellationToken,
) -> anyhow::Result<Vec<(CrateRef, CrateBodies)>> {
    let profile_context = rg_profile::ProfileThreadContext::capture();
    let item_lookup_cache = ItemLookupQueryCache::new();
    // Each worker owns its lowered bodies. Declarations and captured source are immutable,
    // so sparse package selections need no mutable per-package state before dispatch.
    let thread_pool = local_thread_pool("rg-body-resolve")?;
    let results = thread_pool.install(|| {
        packages
            .into_par_iter()
            .map(|(package_slot, package)| {
                let parse_package = parse.package(package_slot.0).with_context(|| {
                    format!("while attempting to fetch parse package {}", package_slot.0)
                })?;
                let _profile_guard = profile_context.enter();
                resolve_package(
                    package_slot,
                    parse_package,
                    package,
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

#[rg_std::cancelable("resolve body package", token = cancellation)]
fn resolve_package(
    package_slot: PackageSlot,
    parse_package: &rg_parse::Package,
    package: LoweredPackageBodies,
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

            CrateBodyBuildState::new(crate_ref, parse_package, crate_bodies, cancellation.clone())
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
