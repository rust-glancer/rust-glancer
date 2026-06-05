//! Resolves lowered Body IR while a build mutator has privileged package access.

use anyhow::Context as _;
use rayon::prelude::*;
use rg_def_map::{DefMapReadTxn, PackageSlot};
use rg_ir_model::TargetRef;
use rg_package_store::PackageStoreError;
use rg_parse::TargetId;
use rg_semantic_ir::SemanticIrReadTxn;

use crate::ir::{PackageBodies, TargetBodiesStatus};

use super::{local_thread_pool, state::TargetBodyBuildState};

pub(super) fn resolve_packages(
    packages: &mut [PackageBodies],
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
) -> anyhow::Result<()> {
    if packages.len() <= 1 {
        for (package_idx, package) in packages.iter_mut().enumerate() {
            resolve_package(PackageSlot(package_idx), package, def_map, semantic_ir)?;
        }
        return Ok(());
    }

    let thread_pool = local_thread_pool("rg-body-resolve")?;
    thread_pool
        .install(|| {
            packages.par_iter_mut().enumerate().try_for_each(
                |(package_idx, package)| -> Result<(), PackageStoreError> {
                    resolve_package(PackageSlot(package_idx), package, def_map, semantic_ir)
                },
            )
        })
        .context("while attempting to resolve body IR packages")
}

pub(super) fn resolve_selected_packages(
    packages: &mut [(PackageSlot, PackageBodies)],
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
) -> anyhow::Result<()> {
    if packages.len() <= 1 {
        for (package_slot, package) in packages {
            resolve_package(*package_slot, package, def_map, semantic_ir)?;
        }
        return Ok(());
    }

    let thread_pool = local_thread_pool("rg-body-resolve")?;
    thread_pool
        .install(|| {
            packages
                .par_iter_mut()
                .try_for_each(|entry| -> Result<(), PackageStoreError> {
                    let package_slot = entry.0;
                    resolve_package(package_slot, &mut entry.1, def_map, semantic_ir)
                })
        })
        .context("while attempting to resolve selected body IR packages")
}

fn resolve_package(
    package_slot: PackageSlot,
    package: &mut PackageBodies,
    def_map_txn: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
) -> Result<(), PackageStoreError> {
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

        TargetBodyBuildState::new(target_ref, target).resolve(def_map_txn, semantic_ir)?;
    }

    Ok(())
}
