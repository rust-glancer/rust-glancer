//! Mechanical lowering from function-body AST into Body IR.
//!
//! This pass intentionally does not resolve names. It records the source shape, lexical scopes,
//! and visibility-order binding boundaries so the later resolution pass can stay focused.

mod expr;
mod function;
mod pat;
mod stmt;
mod syntax;
mod target;

use anyhow::Context as _;
use rayon::prelude::*;

use rg_def_map::PackageSlot;
use rg_ir_model::{FunctionRef, TargetRef};
use rg_parse::{FileId, ParseDb, Span, TargetId};
use rg_semantic_ir::SemanticIrReadTxn;
use rg_text::{NameInterner, PackageNameInterners};

use crate::{
    BodyIrBuildPolicy, BodyIrFile,
    ir::{PackageBodies, TargetBodies},
};

use self::target::TargetLowering;
use super::local_thread_pool;

pub(super) fn build_packages(
    parse: &ParseDb,
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
        semantic_ir,
        BodyIrLoweringScope::PackagePolicy(policy),
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
    semantic_ir: &SemanticIrReadTxn<'_>,
    scope: BodyIrLoweringScope<'_>,
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
    semantic_ir: &SemanticIrReadTxn<'_>,
    scope: BodyIrLoweringScope<'_>,
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

    let selected_count = selected.iter().filter(|selected| **selected).count();
    if selected_count <= 1 {
        build_package_outputs_serial(parse, semantic_ir, scope, interners, selected, packages)
    } else {
        build_package_outputs_parallel(parse, semantic_ir, scope, interners, selected, packages)
    }
}

fn build_package_outputs_serial(
    parse: &ParseDb,
    semantic_ir: &SemanticIrReadTxn<'_>,
    scope: BodyIrLoweringScope<'_>,
    interners: &mut PackageNameInterners,
    selected: &[bool],
    packages: &mut [Option<PackageBodies>],
) -> anyhow::Result<()> {
    for (package_idx, (((parse_package, interner), selected), output)) in parse
        .packages()
        .iter()
        .zip(interners.packages_mut().iter_mut())
        .zip(selected)
        .zip(packages.iter_mut())
        .enumerate()
    {
        if !*selected {
            continue;
        }

        let package = PackageSlot(package_idx);
        *output = Some(build_package_with_interner(
            parse_package,
            semantic_ir,
            scope,
            package,
            interner,
        )?);
    }

    Ok(())
}

fn build_package_outputs_parallel(
    parse: &ParseDb,
    semantic_ir: &SemanticIrReadTxn<'_>,
    scope: BodyIrLoweringScope<'_>,
    interners: &mut PackageNameInterners,
    selected: &[bool],
    packages: &mut [Option<PackageBodies>],
) -> anyhow::Result<()> {
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
    semantic_ir: &SemanticIrReadTxn<'_>,
    scope: BodyIrLoweringScope<'_>,
    package: PackageSlot,
    interner: &mut NameInterner,
) -> anyhow::Result<PackageBodies> {
    let package_ir = semantic_ir.package(package).with_context(|| {
        format!(
            "while attempting to fetch semantic IR package {} for body lowering",
            package.0,
        )
    })?;
    let target_count = package_ir.into_ref().targets().len();
    let mut targets = Vec::with_capacity(target_count);

    for target_idx in 0..target_count {
        let target_ref = TargetRef {
            package,
            target: TargetId(target_idx),
        };
        let functions = semantic_ir
            .functions(target_ref)
            .with_context(|| {
                format!("while attempting to fetch semantic IR functions for target {target_idx}")
            })?
            .into_iter()
            .map(|(function_ref, function)| (function_ref, function.source.file_id, function.span))
            .collect::<Vec<_>>();
        let function_count = functions.len();
        if !scope.should_lower_package(package, parse_package)
            || !scope.should_lower_target(package, &functions)
        {
            targets.push(TargetBodies::skipped(function_count));
            continue;
        }

        targets.push(
            TargetLowering {
                parse_package,
                semantic_ir,
                scope,
                package,
                functions,
                target_bodies: TargetBodies::new(function_count),
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

#[derive(Debug, Clone, Copy)]
pub(super) enum BodyIrLoweringScope<'a> {
    PackagePolicy(BodyIrBuildPolicy),
    SelectedFiles(&'a [BodyIrFile]),
}

impl BodyIrLoweringScope<'_> {
    fn should_lower_package(self, package: PackageSlot, parse_package: &rg_parse::Package) -> bool {
        match self {
            Self::PackagePolicy(policy) => policy.should_lower_package(parse_package),
            Self::SelectedFiles(files) => files.iter().any(|file| file.package == package),
        }
    }

    fn should_lower_target(
        self,
        package: PackageSlot,
        functions: &[(FunctionRef, FileId, Span)],
    ) -> bool {
        match self {
            Self::PackagePolicy(_) => true,
            Self::SelectedFiles(files) => functions.iter().any(|(_, file_id, _)| {
                files
                    .iter()
                    .any(|file| file.package == package && file.file == *file_id)
            }),
        }
    }

    fn should_lower_function(self, package: PackageSlot, file_id: FileId) -> bool {
        match self {
            Self::PackagePolicy(_) => true,
            Self::SelectedFiles(files) => files
                .iter()
                .any(|file| file.package == package && file.file == file_id),
        }
    }
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
    scope: &BodyIrLoweringScope<'_>,
) -> anyhow::Result<()> {
    let BodyIrLoweringScope::SelectedFiles(files) = scope else {
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
