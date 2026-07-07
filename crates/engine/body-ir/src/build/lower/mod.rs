//! Mechanical lowering from AST expression bodies into Body IR.
//!
//! This pass does not resolve names. It records the source shape, lexical scopes,
//! and visibility-order binding boundaries so the later resolution pass can stay focused.

mod body;
mod expr;
mod macro_expansion;
mod pat;
mod stmt;
mod syntax;
mod target;
mod task;

use anyhow::Context as _;
use rayon::prelude::*;

use rg_cfg_eval::CfgEvaluator;
use rg_def_map::{DefMapReadTxn, PackageSlot};
use rg_ir_model::{ConstRef, StaticRef, TargetRef};
use rg_parse::{ParseDb, TargetId};
use rg_semantic_ir::SemanticIrReadTxn;
use rg_text::{NameInterner, PackageNameInterners};

use crate::{BodyIrBuildPolicy, PackageBodies, TargetBodies, TargetBodiesCoverage};

pub(super) use self::macro_expansion::BodyMacroExpansion;
use self::target::TargetLowering;
pub(super) use self::task::{BodyLoweringTask, BodyTaskLowering};
use super::{local_thread_pool, materialization::BodyIrMaterialization};

pub(super) fn build_packages(
    parse: &ParseDb,
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    package_count: usize,
    policy: BodyIrBuildPolicy,
    interners: &mut PackageNameInterners,
) -> anyhow::Result<Vec<PackageBodies>> {
    validate_package_inputs(parse, package_count, interners)?;

    let selected = vec![true; package_count];
    let mut packages = Vec::new();
    packages.resize_with(package_count, || None);
    build_package_outputs(
        parse,
        def_map,
        semantic_ir,
        BodyIrMaterialization::ConfiguredBodies(policy),
        interners,
        &selected,
        &mut packages,
    )?;

    Ok(packages
        .into_iter()
        .map(|package| package.expect("all body IR package slots should be lowered"))
        .collect())
}

pub(super) fn build_selected_packages(
    parse: &ParseDb,
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    scope: BodyIrMaterialization<'_>,
    package_slots: &[PackageSlot],
    interners: &mut PackageNameInterners,
) -> anyhow::Result<Vec<(PackageSlot, PackageBodies)>> {
    validate_package_inputs(parse, parse.package_count(), interners)?;
    validate_selected_packages(parse.package_count(), package_slots)?;
    validate_selected_files(parse.package_count(), &scope)?;

    let mut selected = vec![false; parse.package_count()];
    for package_slot in package_slots {
        selected[package_slot.0] = true;
    }

    let mut packages = Vec::new();
    packages.resize_with(parse.package_count(), || None);
    build_package_outputs(
        parse,
        def_map,
        semantic_ir,
        scope,
        interners,
        &selected,
        &mut packages,
    )?;

    Ok(packages
        .into_iter()
        .enumerate()
        .filter_map(|(package_idx, bodies)| bodies.map(|bodies| (PackageSlot(package_idx), bodies)))
        .collect())
}

fn build_package_outputs(
    parse: &ParseDb,
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    scope: BodyIrMaterialization<'_>,
    interners: &mut PackageNameInterners,
    selected: &[bool],
    packages: &mut [Option<PackageBodies>],
) -> anyhow::Result<()> {
    anyhow::ensure!(
        selected.len() == parse.package_count(),
        "body IR package selection count {} does not match parse package count {}",
        selected.len(),
        parse.package_count(),
    );

    let thread_pool = local_thread_pool("rg-body-lower")?;

    // Body lowering is package-local: each worker receives one parse package, one name interner,
    // and one output slot. Non-selected rebuild slots stay absent from this temporary output.
    thread_pool.install(|| {
        parse
            .packages()
            .par_iter()
            .zip(interners.packages_mut().par_iter_mut())
            .zip(selected.par_iter())
            .zip(packages.par_iter_mut())
            .enumerate()
            .try_for_each(
                |(package_idx, (((parse_package, interner), selected), output))| -> anyhow::Result<()> {
                    if !*selected {
                        return Ok(());
                    }

                    let package = PackageSlot(package_idx);
                    *output = Some(build_package_with_interner(
                        parse_package,
                        def_map,
                        semantic_ir,
                        scope,
                        package,
                        interner,
                    )?);
                    Ok(())
                },
            )
    })
}

fn build_package_with_interner(
    parse_package: &rg_parse::Package,
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    scope: BodyIrMaterialization<'_>,
    package: PackageSlot,
    interner: &mut NameInterner,
) -> anyhow::Result<PackageBodies> {
    let package_ir = semantic_ir.package(package).with_context(|| {
        format!(
            "while attempting to fetch semantic IR package {} for body lowering",
            package.0,
        )
    })?;
    let target_count = package_ir.targets().len();
    let mut targets = Vec::with_capacity(target_count);

    // Go through all targets
    for target_idx in 0..target_count {
        let target_id = TargetId(target_idx);

        // Build cfg evaluator to support `#[cfg]` in bodies
        let parse_target = parse_package.target(target_id).with_context(|| {
            format!("while attempting to fetch parsed target {target_idx} for body lowering")
        })?;
        let target_ref = TargetRef {
            package,
            target: target_id,
        };
        let cfg = CfgEvaluator::new(parse_package.cfg_options(), parse_target.enables_test_cfg());

        // Collect known semantic items.
        let store = semantic_ir
            .items(target_ref)
            .with_context(|| {
                format!("while attempting to fetch semantic IR items for target {target_idx}")
            })?
            .context("store must be present")?;
        let functions = store
            .functions_with_refs()
            .map(|(function_ref, function)| (function_ref, function.source.file_id, function.span))
            .collect::<Vec<_>>();
        let consts = store
            .consts()
            .iter_with_ids()
            .map(|(id, data)| {
                (
                    ConstRef {
                        origin: store.origin(),
                        id,
                    },
                    data.source.file_id,
                    data.span,
                )
            })
            .collect::<Vec<_>>();
        let statics = store
            .statics()
            .iter_with_ids()
            .map(|(id, data)| {
                (
                    StaticRef {
                        origin: store.origin(),
                        id,
                    },
                    data.source.file_id,
                    data.span,
                )
            })
            .collect::<Vec<_>>();

        // Decide both whether this target needs work and how much of its body surface the result
        // will cover. Selected-file rebuilds are allowed to materialize a target partially, while
        // package-policy builds keep the historical all-or-nothing behavior.
        let body_files = functions
            .iter()
            .map(|(_, file_id, _)| *file_id)
            .chain(consts.iter().map(|(_, file_id, _)| *file_id))
            .chain(statics.iter().map(|(_, file_id, _)| *file_id))
            .collect::<Vec<_>>();
        let coverage = scope.target_coverage(package, parse_package, &body_files);
        if !coverage.is_materialized() {
            targets.push(match coverage {
                TargetBodiesCoverage::Missing => TargetBodies::missing(),
                TargetBodiesCoverage::SkippedByPolicy => TargetBodies::skipped_by_policy(),
                TargetBodiesCoverage::Complete | TargetBodiesCoverage::Partial => {
                    unreachable!("materialized body IR coverage should be lowered")
                }
            });
            continue;
        }

        // Lower.
        targets.push(
            TargetLowering {
                parse_package,
                def_map,
                semantic_ir,
                scope,
                package,
                functions,
                consts,
                statics,
                target_bodies: TargetBodies::with_coverage(coverage),
                cfg,
                interner,
            }
            .lower()
            .with_context(|| {
                format!("while attempting to lower body IR for target {target_idx}")
            })?,
        );
    }

    Ok(PackageBodies::new(targets))
}

fn validate_package_inputs(
    parse: &ParseDb,
    package_count: usize,
    interners: &PackageNameInterners,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        parse.package_count() == package_count,
        "parse package count {} does not match body IR package count {}",
        parse.package_count(),
        package_count,
    );
    anyhow::ensure!(
        interners.package_count() == package_count,
        "name interner count {} does not match body IR package count {}",
        interners.package_count(),
        package_count,
    );

    Ok(())
}

fn validate_selected_packages(
    package_count: usize,
    package_slots: &[PackageSlot],
) -> anyhow::Result<()> {
    if let Some(package) = package_slots
        .iter()
        .copied()
        .find(|package| package.0 >= package_count)
    {
        anyhow::bail!(
            "body IR package slot {} is out of bounds for {package_count} parsed packages",
            package.0,
        );
    }

    Ok(())
}

fn validate_selected_files(
    package_count: usize,
    scope: &BodyIrMaterialization<'_>,
) -> anyhow::Result<()> {
    let BodyIrMaterialization::SelectedFiles(files) = scope else {
        return Ok(());
    };

    if let Some(file) = files
        .iter()
        .copied()
        .find(|file| file.package.0 >= package_count)
    {
        anyhow::bail!(
            "body IR file package slot {} is out of bounds for {package_count} parsed packages",
            file.package.0,
        );
    }

    Ok(())
}
