//! Resolves lowered Body IR while a build mutator has privileged package access.

use anyhow::Context as _;
use rayon::prelude::*;
use rg_def_map::{DefMapReadTxn, PackageSlot};
use rg_ir_model::TargetRef;
use rg_parse::TargetId;
use rg_semantic_ir::SemanticIrReadTxn;
use rg_text::{NameInterner, PackageNameInterners};

use crate::ir::{PackageBodies, TargetBodiesStatus};

use super::{local_thread_pool, state::TargetBodyBuildState};

pub(super) fn resolve_packages(
    packages: &mut [PackageBodies],
    parse: &rg_parse::ParseDb,
    interners: &mut PackageNameInterners,
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
) -> anyhow::Result<()> {
    if packages.len() <= 1 {
        for (package_idx, ((package, parse_package), interner)) in packages
            .iter_mut()
            .zip(parse.packages())
            .zip(interners.packages_mut())
            .enumerate()
        {
            resolve_package(
                PackageSlot(package_idx),
                parse_package,
                package,
                interner,
                def_map,
                semantic_ir,
            )?;
        }
        return Ok(());
    }

    let thread_pool = local_thread_pool("rg-body-resolve")?;
    thread_pool
        .install(|| {
            packages
                .par_iter_mut()
                .zip(parse.packages().par_iter())
                .zip(interners.packages_mut().par_iter_mut())
                .enumerate()
                .try_for_each(
                    |(package_idx, ((package, parse_package), interner))| -> anyhow::Result<()> {
                        resolve_package(
                            PackageSlot(package_idx),
                            parse_package,
                            package,
                            interner,
                            def_map,
                            semantic_ir,
                        )
                    },
                )
        })
        .context("while attempting to resolve body IR packages")
}

pub(super) fn resolve_selected_packages(
    packages: &mut [(PackageSlot, PackageBodies)],
    parse: &rg_parse::ParseDb,
    interners: &mut PackageNameInterners,
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
) -> anyhow::Result<()> {
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
        jobs.push((*package_slot, parse_package, package, interner));
        remaining_interners = rest;
        next_package_idx = package_slot.0 + 1;
    }

    if jobs.len() <= 1 {
        for (package_slot, parse_package, package, interner) in jobs {
            resolve_package(
                package_slot,
                parse_package,
                package,
                interner,
                def_map,
                semantic_ir,
            )?;
        }
        return Ok(());
    }

    let thread_pool = local_thread_pool("rg-body-resolve")?;
    thread_pool
        .install(|| {
            jobs.into_par_iter().try_for_each(
                |(package_slot, parse_package, package, interner)| -> anyhow::Result<()> {
                    resolve_package(
                        package_slot,
                        parse_package,
                        package,
                        interner,
                        def_map,
                        semantic_ir,
                    )
                },
            )
        })
        .context("while attempting to resolve selected body IR packages")
}

fn resolve_package(
    package_slot: PackageSlot,
    parse_package: &rg_parse::Package,
    package: &mut PackageBodies,
    interner: &mut NameInterner,
    def_map_txn: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
) -> anyhow::Result<()> {
    // Resolution is a mutation pass over already-lowered bodies. Skipped targets intentionally
    // keep their body stores empty so dependency body internals stay cheap by default.
    for (target_idx, target) in package.targets_mut().iter_mut().enumerate() {
        if matches!(target.status(), TargetBodiesStatus::Skipped) {
            continue;
        }

        let target_ref = TargetRef {
            package: package_slot,
            target: TargetId(target_idx),
        };

        TargetBodyBuildState::new(target_ref, parse_package, target, interner)
            .resolve(def_map_txn, semantic_ir)?;
    }

    Ok(())
}
