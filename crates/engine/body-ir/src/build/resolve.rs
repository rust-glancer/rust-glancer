//! Resolves lowered Body IR while a build mutator has privileged package access.

use anyhow::Context as _;
use rayon::prelude::*;
use rg_def_map::{DefMapReadTxn, PackageSlot};
use rg_ir_model::{BodyId, BodyRef, TargetRef};
use rg_package_store::PackageStoreError;
use rg_parse::TargetId;
use rg_semantic_ir::SemanticIrReadTxn;

use crate::{
    ir::{
        PackageBodies, TargetBodiesStatus,
        body_map::{BodyDefMapCollector, BodyItemStoreCollector},
    },
    resolution::{BodyResolver, SemanticResolutionIndex},
};

use super::local_thread_pool;

pub(super) fn resolve_packages(
    packages: &mut [PackageBodies],
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
) -> anyhow::Result<()> {
    let semantic_index = SemanticResolutionIndex::build(semantic_ir)
        .context("while attempting to build semantic index for body resolution")?;

    if packages.len() <= 1 {
        for (package_idx, package) in packages.iter_mut().enumerate() {
            resolve_package(
                PackageSlot(package_idx),
                package,
                def_map,
                semantic_ir,
                &semantic_index,
            )?;
        }
        return Ok(());
    }

    let thread_pool = local_thread_pool("rg-body-resolve")?;
    thread_pool
        .install(|| {
            packages.par_iter_mut().enumerate().try_for_each(
                |(package_idx, package)| -> Result<(), PackageStoreError> {
                    resolve_package(
                        PackageSlot(package_idx),
                        package,
                        def_map,
                        semantic_ir,
                        &semantic_index,
                    )
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
    let semantic_index = SemanticResolutionIndex::build(semantic_ir)
        .context("while attempting to build semantic index for body resolution")?;

    if packages.len() <= 1 {
        for (package_slot, package) in packages {
            resolve_package(
                *package_slot,
                package,
                def_map,
                semantic_ir,
                &semantic_index,
            )?;
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
                    resolve_package(
                        package_slot,
                        &mut entry.1,
                        def_map,
                        semantic_ir,
                        &semantic_index,
                    )
                })
        })
        .context("while attempting to resolve selected body IR packages")
}

fn resolve_package(
    package_slot: PackageSlot,
    package: &mut PackageBodies,
    def_map_txn: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    semantic_index: &SemanticResolutionIndex,
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
        let target_def_map = def_map_txn
            .def_map(target_ref)?
            .expect("Target DefMap must be present");

        for (body_idx, body) in target.bodies_mut().iter_mut().enumerate() {
            let body_ref = BodyRef {
                target: target_ref,
                body: BodyId(body_idx),
            };
            // First, collect the defmap
            let body_def_map = BodyDefMapCollector::new(target_def_map, body_ref, body).collect();
            // Then, collect the local items
            let body_item_store = BodyItemStoreCollector::new(body, &body_def_map).collect();
            // TODO: Note that there is no resolution for both defmap and local items just yet.
            body.body_def_map = Some(body_def_map);
            body.body_item_store = Some(body_item_store);

            BodyResolver::new(def_map_txn, semantic_ir, semantic_index, body_ref, body)
                .resolve()?;
        }
    }

    Ok(())
}
