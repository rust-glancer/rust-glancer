//! Builds and rebuilds Body IR snapshots.

mod body_def_map;
mod body_item_store;
mod lower;
mod materialization;
mod pattern_binding;
mod query_source;
mod resolve;
mod state;

use std::{num::NonZeroUsize, time::Instant};

use anyhow::Context as _;

use rg_def_map::PackageDefMaps as DefMapPackage;
use rg_def_map::PackageSlot;
use rg_ir_model::{CrateId, CrateRef};
use rg_package_store::{PackageLoader, PackageSubset};
use rg_semantic_ir::PackageIr;
use rg_std::{Shrink, UniqueVec};
use rg_text::PackageNameInterners;

use crate::{BodyIrBuildPolicy, BodyIrDb, BodyIrFile, PackageBodies};

use self::materialization::BodyIrMaterializationPlan;

pub use self::lower::{
    CurrentBodyBuildCheckpoint, CurrentBodyBuildOutcome, CurrentBodyBuilder, CurrentBodySelection,
    CurrentBodyUnavailable,
};

/// Package-local stage of one Body IR build.
///
/// Lowering records source structure first. Resolution then attaches the semantic facts used by
/// queries. Reporting these as separate stages avoids presenting package counts as an elapsed-time
/// percentage: the two kinds of work can have very different costs for the same package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyIrBuildStage {
    Lowering,
    Resolving,
}

/// Completed package count within one [`BodyIrBuildStage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyIrBuildProgress {
    stage: BodyIrBuildStage,
    completed_packages: usize,
    total_packages: usize,
}

impl BodyIrBuildProgress {
    fn new(stage: BodyIrBuildStage, completed_packages: usize, total_packages: usize) -> Self {
        Self {
            stage,
            completed_packages,
            total_packages,
        }
    }

    pub fn stage(self) -> BodyIrBuildStage {
        self.stage
    }

    pub fn completed_packages(self) -> usize {
        self.completed_packages
    }

    pub fn total_packages(self) -> usize {
        self.total_packages
    }
}

/// Builds selected Body IR packages on top of one baseline snapshot.
///
/// Fresh construction and saved updates differ only in the baseline and selected package set.
/// Keeping both on this builder makes materialization, lazy reads, worker limits, and compaction
/// follow one path. Callers must choose one materialization mode before building.
pub struct BodyIrDbBuilder<'db, 'names> {
    baseline: &'db BodyIrDb,
    parse: &'db rg_parse::ParseDb,
    def_map: &'db rg_def_map::DefMapDb,
    semantic_ir: &'db rg_semantic_ir::SemanticIrDb,
    materialization: Option<BodyIrMaterializationPlan>,
    packages: &'db [PackageSlot],
    interners: &'names mut PackageNameInterners,
    def_map_loader: PackageLoader<'db, DefMapPackage>,
    semantic_ir_loader: PackageLoader<'db, PackageIr>,
    subset: &'db PackageSubset,
    worker_limit: Option<NonZeroUsize>,
}

impl<'db, 'names> BodyIrDbBuilder<'db, 'names> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        baseline: &'db BodyIrDb,
        parse: &'db rg_parse::ParseDb,
        def_map: &'db rg_def_map::DefMapDb,
        semantic_ir: &'db rg_semantic_ir::SemanticIrDb,
        packages: &'db [PackageSlot],
        interners: &'names mut PackageNameInterners,
        def_map_loader: PackageLoader<'db, DefMapPackage>,
        semantic_ir_loader: PackageLoader<'db, PackageIr>,
        subset: &'db PackageSubset,
    ) -> Self {
        Self {
            baseline,
            parse,
            def_map,
            semantic_ir,
            materialization: None,
            packages,
            interners,
            def_map_loader,
            semantic_ir_loader,
            subset,
            worker_limit: None,
        }
    }

    /// Lowers body contents selected by the package-and-target policy.
    pub fn configured_bodies(mut self, policy: BodyIrBuildPolicy) -> Self {
        self.materialization = Some(BodyIrMaterializationPlan::ConfiguredBodies(policy));
        self
    }

    /// Builds coverage records without lowering body contents selected by the policy.
    pub fn coverage_only(mut self, policy: BodyIrBuildPolicy) -> Self {
        self.materialization = Some(BodyIrMaterializationPlan::CoverageOnly(policy));
        self
    }

    /// Rebuilds bodies belonging to exact semantic interpretations of selected files.
    pub fn selected_files(mut self, files: Vec<BodyIrFile>) -> Self {
        self.materialization = Some(BodyIrMaterializationPlan::SelectedFiles(files));
        self
    }

    /// Rebuild complete Body IR for exact semantic crates without selecting sibling targets.
    pub fn selected_crates(mut self, crates: UniqueVec<CrateRef>) -> Self {
        self.materialization = Some(BodyIrMaterializationPlan::SelectedCrates(crates));
        self
    }

    /// Bounds package-level lowering and resolution pools for this build.
    ///
    /// `None` keeps Rayon's machine-default pool width. A limit is useful for indexing modes that
    /// prefer lower per-worker allocation overlap over maximum package throughput.
    pub fn worker_limit(mut self, worker_limit: Option<NonZeroUsize>) -> Self {
        self.worker_limit = worker_limit;
        self
    }

    pub fn build(self) -> anyhow::Result<BodyIrDb> {
        self.build_with_optional_package_priority(None, &|_, _| {}, None)
    }

    /// Build every selected package once, with observations for the detached-indexing caller.
    ///
    /// `publish_priority` receives a compact copy when a package requested by
    /// `priority_packages` resolves. The ordinary build still retains all resolved packages until
    /// its two-phase compaction, so publication does not change the final database or split one
    /// build into cache-cold sub-builds.
    ///
    /// `report_progress` describes package completion separately for lowering and resolution. It
    /// is an observation of this build, not a scheduling hook: callers should return promptly and
    /// move any editor or RPC work onto their own queue.
    pub fn build_with_package_priority(
        self,
        priority_packages: &(dyn Fn() -> Vec<PackageSlot> + Sync),
        publish_priority: &(dyn Fn(PackageSlot, PackageBodies) + Sync),
        report_progress: &(dyn Fn(BodyIrBuildProgress) + Sync),
    ) -> anyhow::Result<BodyIrDb> {
        self.build_with_optional_package_priority(
            Some(priority_packages),
            publish_priority,
            Some(report_progress),
        )
    }

    fn build_with_optional_package_priority(
        self,
        priority_packages: Option<&(dyn Fn() -> Vec<PackageSlot> + Sync)>,
        publish_priority: &(dyn Fn(PackageSlot, PackageBodies) + Sync),
        report_progress: Option<&(dyn Fn(BodyIrBuildProgress) + Sync)>,
    ) -> anyhow::Result<BodyIrDb> {
        // 1. Start with the baseline snapshot so untouched resident payloads remain shared and
        // offloaded slots keep their coverage summaries. The read transactions may load
        // dependencies from the bounded subset while selected packages are rebuilt in memory.
        let build_started = Instant::now();
        let clone_started = Instant::now();
        let mut next = self.baseline.clone();
        let clone_ms = clone_started.elapsed().as_millis();
        let setup_started = Instant::now();
        let packages = normalized_package_slots(self.packages);
        let materialization = self
            .materialization
            .as_ref()
            .context("Body IR package build requires a materialization selection")?
            .lowering();
        let semantic_ir_txn = self
            .semantic_ir
            .read_txn_for_subset(self.semantic_ir_loader, self.subset);
        let def_map_txn = self
            .def_map
            .read_txn_for_subset(self.def_map_loader, self.subset);
        let setup_ms = setup_started.elapsed().as_millis();

        // 2. Lower only the requested bodies. Crates whose resulting coverage is unmaterialized
        // remain in the package shape, but skip lookup-query construction and body resolution.
        let lowering_started = Instant::now();
        if let Some(report_progress) = report_progress.filter(|_| !packages.is_empty()) {
            report_progress(BodyIrBuildProgress::new(
                BodyIrBuildStage::Lowering,
                0,
                packages.len(),
            ));
        }
        let rebuilt_packages = lower::build_selected_packages(
            self.parse,
            &def_map_txn,
            &semantic_ir_txn,
            materialization,
            &packages,
            self.interners,
            self.worker_limit,
            report_progress,
        )
        .context("while attempting to lower selected body IR packages")?;
        let lowering_ms = lowering_started.elapsed().as_millis();

        // 3. Resolve the lowered bodies, then compact the selected packages while the temporary
        // allocations made by this build are still grouped together.
        let resolution_started = Instant::now();
        if let Some(report_progress) = report_progress.filter(|_| !packages.is_empty()) {
            report_progress(BodyIrBuildProgress::new(
                BodyIrBuildStage::Resolving,
                0,
                packages.len(),
            ));
        }
        let rebuilt_packages = resolve::resolve_selected_packages(
            rebuilt_packages,
            self.parse,
            self.interners,
            &def_map_txn,
            &semantic_ir_txn,
            priority_packages,
            publish_priority,
            self.worker_limit,
            report_progress,
        )
        .context("while attempting to resolve selected body IR packages")?;
        let resolution_ms = resolution_started.elapsed().as_millis();
        let compaction_started = Instant::now();
        let compacted_packages = compact_rebuilt_packages_two_phase(rebuilt_packages);
        let compaction_ms = compaction_started.elapsed().as_millis();

        // 4. Replace package slots only after every fallible build phase has succeeded, then close
        // the read views before returning.
        let replacement_started = Instant::now();
        {
            let mut mutator = next.mutator();
            for (package, rebuilt) in compacted_packages {
                let rebuilt =
                    retain_unselected_crates(self.baseline, package, rebuilt, materialization)?;
                mutator.replace_package(package, rebuilt).with_context(|| {
                    format!("while attempting to replace body IR package {}", package.0)
                })?;
            }
        }
        let replacement_ms = replacement_started.elapsed().as_millis();
        let read_txn_drop_started = Instant::now();
        drop(semantic_ir_txn);
        drop(def_map_txn);
        let read_txn_drop_ms = read_txn_drop_started.elapsed().as_millis();

        tracing::trace!(
            ?materialization,
            package_count = packages.len(),
            worker_limit = self.worker_limit.map(NonZeroUsize::get),
            clone_ms,
            setup_ms,
            lowering_ms,
            resolution_ms,
            compaction_ms,
            replacement_ms,
            read_txn_drop_ms,
            total_ms = build_started.elapsed().as_millis(),
            "Body IR package build phases finished"
        );
        Ok(next)
    }
}

/// Preserve sibling target payloads during an exact file- or crate-selected rebuild.
///
/// Body IR storage stays package-shaped, but selection is semantic-crate-shaped. Replacing only
/// the selected crate slots prevents one integration test or example from resetting every other
/// target in the same Cargo package.
fn retain_unselected_crates(
    baseline: &BodyIrDb,
    package: PackageSlot,
    rebuilt: PackageBodies,
    materialization: materialization::BodyIrMaterialization<'_>,
) -> anyhow::Result<PackageBodies> {
    if matches!(
        materialization,
        materialization::BodyIrMaterialization::ConfiguredBodies(_)
            | materialization::BodyIrMaterialization::CoverageOnly(_)
    ) {
        return Ok(rebuilt);
    }

    let previous = baseline.resident_package(package).with_context(|| {
        format!(
            "exact Body IR rebuild requires resident package {}",
            package.0
        )
    })?;
    anyhow::ensure!(
        previous.crates().len() == rebuilt.crates().len(),
        "exact Body IR rebuild changed package {} crate count from {} to {}",
        package.0,
        previous.crates().len(),
        rebuilt.crates().len(),
    );

    let crates = rebuilt
        .crates()
        .iter()
        .enumerate()
        .map(|(crate_idx, crate_bodies)| {
            let crate_ref = CrateRef {
                package,
                crate_id: CrateId(crate_idx),
            };
            if materialization.selects_crate(crate_ref) {
                crate_bodies.clone()
            } else {
                previous.crates()[crate_idx].clone()
            }
        })
        .collect();
    Ok(PackageBodies::new(crates))
}

fn compact_rebuilt_packages_two_phase(
    rebuilt_packages: Vec<(PackageSlot, PackageBodies)>,
) -> Vec<(PackageSlot, PackageBodies)> {
    let compacted = rebuilt_packages
        .iter()
        .map(|(package, rebuilt)| (*package, compact_package_copy(rebuilt)))
        .collect::<Vec<_>>();
    drop(rebuilt_packages);
    compacted
}

fn compact_package_copy(package: &PackageBodies) -> PackageBodies {
    let mut compacted = package.clone();
    Shrink::shrink_to_fit(&mut compacted);
    compacted
}

fn local_thread_pool(
    thread_name_prefix: &'static str,
    worker_limit: Option<NonZeroUsize>,
) -> anyhow::Result<rayon::ThreadPool> {
    let mut builder = rayon::ThreadPoolBuilder::new()
        .thread_name(move |index| format!("{thread_name_prefix}-{index}"));
    if let Some(worker_limit) = worker_limit {
        let worker_count = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(worker_limit.get())
            .min(worker_limit.get());
        builder = builder.num_threads(worker_count);
    }

    builder
        .build()
        .with_context(|| format!("while attempting to create {thread_name_prefix} thread pool"))
}

fn normalized_package_slots(packages: &[PackageSlot]) -> Vec<PackageSlot> {
    let mut slots = packages.to_vec();
    slots.sort_by_key(|slot| slot.0);
    slots.dedup();
    slots
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use super::local_thread_pool;

    #[test]
    fn body_ir_thread_pool_honors_worker_limit() {
        let worker_limit = NonZeroUsize::new(2).expect("test worker limit should be non-zero");
        let expected_workers = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(worker_limit.get())
            .min(worker_limit.get());

        let thread_pool = local_thread_pool("rg-body-test", Some(worker_limit))
            .expect("limited Body IR thread pool should build");

        assert_eq!(thread_pool.current_num_threads(), expected_workers);
    }
}
