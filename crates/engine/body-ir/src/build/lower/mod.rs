//! Mechanical lowering from AST expression bodies into Body IR.
//!
//! This pass does not resolve names. It records the source shape, lexical scopes,
//! and visibility-order binding boundaries so the later resolution pass can stay focused.

use std::time::{Duration, Instant};

mod body;
mod builder;
mod crate_lowering;
mod expr;
mod macro_expansion;
mod pat;
mod stmt;
mod syntax;
mod syntax_kind;
mod task;

use anyhow::Context as _;
use rayon::prelude::*;

use rg_arena::Arena;
use rg_cfg_eval::CfgEvaluator;
use rg_def_map::{DefMapReadTxn, PackageSlot};
use rg_ir_model::{BodyId, ConstRef, CrateId, CrateRef, StaticRef};
use rg_parse::ParseDb;
use rg_semantic_ir::SemanticIrReadTxn;
use rg_text::{NameInterner, PackageNameInterners};

use crate::{BodyIrBuildPolicy, CrateBodiesCoverage};

pub(super) use self::builder::{LoweredBodyData, PendingBindingResolution};
use self::crate_lowering::CrateLowering;
pub(super) use self::macro_expansion::BodyMacroExpansion;
pub(super) use self::task::{BodyLoweringTask, BodyTaskLowering};
use super::{local_thread_pool, materialization::BodyIrMaterialization};

// These thresholds are diagnostic filters, not build budgets. Debug logging should identify
// unusually expensive units without producing one record for every ordinary package or crate.
const SLOW_CRATE_LOWERING: Duration = Duration::from_secs(1);
const SLOW_PACKAGE_LOWERING: Duration = Duration::from_secs(2);

/// Package-local structural lowering output. Semantic facts are intentionally absent here.
pub(super) type LoweredPackageBodies = Vec<LoweredCrateBodies>;

/// Crate-local bodies that still retain source-ambiguous pattern binding candidates.
pub(super) struct LoweredCrateBodies {
    coverage: CrateBodiesCoverage,
    bodies: Arena<BodyId, LoweredBodyData>,
}

impl LoweredCrateBodies {
    pub(super) fn with_coverage(coverage: CrateBodiesCoverage) -> Self {
        Self {
            coverage,
            bodies: Arena::new(),
        }
    }

    pub(super) fn coverage(&self) -> CrateBodiesCoverage {
        self.coverage
    }

    pub(super) fn bodies(&self) -> &Arena<BodyId, LoweredBodyData> {
        &self.bodies
    }

    pub(super) fn bodies_mut(&mut self) -> &mut Arena<BodyId, LoweredBodyData> {
        &mut self.bodies
    }

    pub(super) fn alloc_body(&mut self, body: LoweredBodyData) -> BodyId {
        self.bodies.alloc(body)
    }

    pub(super) fn into_bodies(self) -> Arena<BodyId, LoweredBodyData> {
        self.bodies
    }
}

pub(super) fn build_packages(
    parse: &ParseDb,
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    package_count: usize,
    policy: BodyIrBuildPolicy,
    interners: &mut PackageNameInterners,
) -> anyhow::Result<Vec<LoweredPackageBodies>> {
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
) -> anyhow::Result<Vec<(PackageSlot, LoweredPackageBodies)>> {
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
    packages: &mut [Option<LoweredPackageBodies>],
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
) -> anyhow::Result<LoweredPackageBodies> {
    let span = tracing::debug_span!(
        "body_ir_package_lowering",
        rg.package = parse_package.package_name(),
        rg.package_slot = package.0,
    );
    let _entered = span.enter();
    let started = Instant::now();
    let package_ir = semantic_ir.package(package).with_context(|| {
        format!(
            "while attempting to fetch semantic IR package {} for body lowering",
            package.0,
        )
    })?;
    let crate_count = package_ir.crates().len();
    let mut crates = Vec::with_capacity(crate_count);
    let def_map_package = def_map.package(package).with_context(|| {
        format!(
            "while attempting to fetch def-map package {} for body lowering",
            package.0
        )
    })?;

    // A semantic crate retains the Cargo target that provides its source and cfg context. Keep the
    // conversion at this boundary instead of relying on matching arena indexes in later phases.
    for crate_idx in 0..crate_count {
        let crate_id = CrateId(crate_idx);
        let cargo_target = def_map_package
            .crate_data(crate_id)
            .context("semantic crate should have definition data")?
            .cargo_target();

        // Build cfg evaluator to support `#[cfg]` in bodies
        let parse_target = parse_package.target(cargo_target).with_context(|| {
            format!("while attempting to fetch parsed target for crate {crate_idx}")
        })?;
        let crate_ref = CrateRef { package, crate_id };
        let cfg = CfgEvaluator::new(parse_package.cfg_options(), parse_target.enables_test_cfg());

        // Collect known semantic items.
        let store = semantic_ir
            .items(crate_ref)
            .with_context(|| {
                format!("while attempting to fetch semantic IR items for crate {crate_idx}")
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

        // Decide both whether this crate needs work and how much of its body surface the result
        // will cover. Selected-file rebuilds are allowed to materialize a crate partially, while
        // package-policy builds keep the historical all-or-nothing behavior.
        let body_files = functions
            .iter()
            .map(|(_, file_id, _)| *file_id)
            .chain(consts.iter().map(|(_, file_id, _)| *file_id))
            .chain(statics.iter().map(|(_, file_id, _)| *file_id))
            .collect::<Vec<_>>();
        let coverage = scope.crate_coverage(package, parse_package, &body_files);
        if !coverage.is_materialized() {
            crates.push(LoweredCrateBodies::with_coverage(coverage));
            continue;
        }

        // Lower. Keep the crate identity in the active span so a slow macro/body event emitted by
        // a nested lowering helper can identify its source without adding diagnostic parameters.
        let crate_span = tracing::debug_span!("body_ir_crate_lowering", rg.crate_id = crate_idx,);
        let _crate_entered = crate_span.enter();
        let crate_started = Instant::now();
        let crate_bodies = CrateLowering {
            parse_package,
            def_map,
            semantic_ir,
            scope,
            package,
            functions,
            consts,
            statics,
            crate_bodies: LoweredCrateBodies::with_coverage(coverage),
            cfg,
            interner,
        }
        .lower()
        .with_context(|| format!("while attempting to lower body IR for crate {crate_idx}"))?;
        let crate_elapsed = crate_started.elapsed();
        if crate_elapsed >= SLOW_CRATE_LOWERING {
            tracing::debug!(
                elapsed_ms = crate_elapsed.as_millis(),
                body_count = crate_bodies.bodies().len(),
                "slow Body IR crate lowering"
            );
        }
        crates.push(crate_bodies);
    }

    let elapsed = started.elapsed();
    if elapsed >= SLOW_PACKAGE_LOWERING {
        tracing::debug!(
            elapsed_ms = elapsed.as_millis(),
            crate_count,
            body_count = crates
                .iter()
                .map(|crate_bodies| crate_bodies.bodies().len())
                .sum::<usize>(),
            "slow Body IR package lowering"
        );
    }

    Ok(crates)
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
