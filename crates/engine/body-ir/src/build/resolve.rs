//! Resolves lowered Body IR while a build mutator has privileged package access.

use std::time::{Duration, Instant};

use anyhow::Context as _;
use rayon::prelude::*;
use rg_def_map::{DefMapReadTxn, PackageSlot};
use rg_ir_model::{CrateId, CrateRef};
use rg_semantic_ir::SemanticIrReadTxn;
use rg_text::{NameInterner, PackageNameInterners};
use rg_ty::TraitSelectionSession;

use crate::{CrateBodies, PackageBodies};

use super::{local_thread_pool, lower::LoweredPackageBodies, state::CrateBodyBuildState};

// Package resolution runs in parallel, so report only packages large enough to stand out from
// normal scheduling variance.
const SLOW_PACKAGE_RESOLUTION: Duration = Duration::from_secs(2);

pub(super) struct ResolvedPackages<T> {
    pub(super) packages: T,
    pub(super) trait_selection_sessions: Vec<TraitSelectionSession>,
}

struct ResolvedPackage {
    bodies: PackageBodies,
    trait_selection_sessions: Vec<TraitSelectionSession>,
}

pub(super) fn resolve_packages(
    packages: Vec<LoweredPackageBodies>,
    parse: &rg_parse::ParseDb,
    interners: &mut PackageNameInterners,
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    retain_trait_selection: bool,
) -> anyhow::Result<ResolvedPackages<Vec<PackageBodies>>> {
    let profile_context = rg_profile::ProfileThreadContext::capture();
    let thread_pool = local_thread_pool("rg-body-resolve")?;
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
                        retain_trait_selection,
                    )
                })
                .collect::<anyhow::Result<Vec<_>>>()
        })
        .context("while attempting to resolve body IR packages")?;
    let mut packages = Vec::with_capacity(resolved.len());
    let mut trait_selection_sessions = Vec::new();
    for resolved in resolved {
        packages.push(resolved.bodies);
        trait_selection_sessions.extend(resolved.trait_selection_sessions);
    }
    Ok(ResolvedPackages {
        packages,
        trait_selection_sessions,
    })
}

pub(super) fn resolve_selected_packages(
    packages: Vec<(PackageSlot, LoweredPackageBodies)>,
    parse: &rg_parse::ParseDb,
    interners: &mut PackageNameInterners,
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    retain_trait_selection: bool,
) -> anyhow::Result<ResolvedPackages<Vec<(PackageSlot, PackageBodies)>>> {
    let profile_context = rg_profile::ProfileThreadContext::capture();
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

    let thread_pool = local_thread_pool("rg-body-resolve")?;
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
                        retain_trait_selection,
                    )?;
                    Ok((package_slot, package))
                })
                .collect::<anyhow::Result<Vec<_>>>()
        })
        .context("while attempting to resolve selected body IR packages")?;

    let mut packages = Vec::with_capacity(resolved.len());
    let mut trait_selection_sessions = Vec::new();
    for (package_slot, resolved) in resolved {
        packages.push((package_slot, resolved.bodies));
        trait_selection_sessions.extend(resolved.trait_selection_sessions);
    }
    Ok(ResolvedPackages {
        packages,
        trait_selection_sessions,
    })
}

fn resolve_package(
    package_slot: PackageSlot,
    parse_package: &rg_parse::Package,
    package: LoweredPackageBodies,
    interner: &mut NameInterner,
    def_map_txn: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    retain_trait_selection: bool,
) -> anyhow::Result<ResolvedPackage> {
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
                return Ok((CrateBodies::empty(coverage), None));
            }

            let crate_ref = CrateRef {
                package: package_slot,
                crate_id: CrateId(crate_idx),
            };

            let (bodies, trait_selection) =
                CrateBodyBuildState::new(crate_ref, parse_package, crate_bodies, interner)
                    .resolve(def_map_txn, semantic_ir, retain_trait_selection)?;
            Ok((bodies, trait_selection))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut bodies = Vec::with_capacity(crates.len());
    let mut trait_selection_sessions = Vec::new();
    for (crate_bodies, trait_selection) in crates {
        bodies.push(crate_bodies);
        trait_selection_sessions.extend(trait_selection);
    }

    let elapsed = started.elapsed();
    if elapsed >= SLOW_PACKAGE_RESOLUTION {
        tracing::debug!(
            elapsed_ms = elapsed.as_millis(),
            crate_count,
            body_count = bodies
                .iter()
                .map(|crate_bodies| crate_bodies.bodies().len())
                .sum::<usize>(),
            "slow Body IR package resolution"
        );
    }

    Ok(ResolvedPackage {
        bodies: PackageBodies::new(bodies),
        trait_selection_sessions,
    })
}
