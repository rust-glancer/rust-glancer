//! Builds analysis data for the code inside Rust bodies.
//!
//! [`BodyIrBuilder`] analyzes saved source using declarations already collected by DefMap and
//! Semantic IR. [`CurrentSourceBuilder`] handles editor text for one request, preparing the
//! declarations and bodies needed by that request against the saved project.

mod current;
mod local_items;
mod lower;
mod materialization;
mod pattern_binding;
mod query_source;
mod resolve;
mod state;

use std::{num::NonZeroUsize, sync::Mutex};

use anyhow::Context as _;

use rg_def_map::{DefMapDb, DefMapLoader, PackageSlot};
use rg_ir_model::CrateRef;
use rg_package_store::PackageSubset;
use rg_parse::ParseDb;
use rg_semantic_ir::{SemanticIrDb, SemanticIrLoader};
use rg_std::{Shrink, UniqueVec};
use rg_text::PackageNameInterners;

use crate::{BodyIrBuildPolicy, BodyIrFile, CrateBodies, PackageBodies};

use self::materialization::BodyIrMaterializationPlan;

pub use self::current::{
    CurrentSourceBuildCheckpoint, CurrentSourceBuildSummary, CurrentSourceBuilder,
    CurrentSourceSelection, CurrentSourceUnavailable,
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

/// Builds [`CrateBodies`] from saved source and the declarations in
/// [`DefMapDb`](rg_def_map::DefMapDb) and [`SemanticIrDb`](rg_semantic_ir::SemanticIrDb).
///
/// Select bodies by build policy or by explicit files and crates, then collect the results with
/// [`Self::build`] or receive them package by package with [`Self::build_with_package_priority`].
/// Installing these results in a [`BodyIrDb`](crate::BodyIrDb) is the caller's responsibility,
/// so body analysis can run independently of changes to the project's existing bodies.
pub struct BodyIrBuilder<'db, 'names> {
    parse: &'db ParseDb,
    def_map: &'db DefMapDb,
    semantic_ir: &'db SemanticIrDb,
    materialization: Option<BodyIrMaterializationPlan>,
    packages: &'db [PackageSlot],
    interners: &'names mut PackageNameInterners,
    def_map_loader: DefMapLoader<'db>,
    semantic_ir_loader: SemanticIrLoader<'db>,
    subset: &'db PackageSubset,
    worker_limit: Option<NonZeroUsize>,
    cancellation: rg_std::CancellationToken,
    copy_compact_packages: Vec<PackageSlot>,
}

impl rg_std::Cancelable for BodyIrBuilder<'_, '_> {
    fn check_cancelled(&self, checkpoint: &'static str) -> Result<(), rg_std::Cancelled> {
        rg_std::Cancelable::check_cancelled(&self.cancellation, checkpoint)
    }
}

impl<'db, 'names> BodyIrBuilder<'db, 'names> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        parse: &'db ParseDb,
        def_map: &'db DefMapDb,
        semantic_ir: &'db SemanticIrDb,
        packages: &'db [PackageSlot],
        copy_compact_packages: &[PackageSlot],
        interners: &'names mut PackageNameInterners,
        def_map_loader: DefMapLoader<'db>,
        semantic_ir_loader: SemanticIrLoader<'db>,
        subset: &'db PackageSubset,
    ) -> Self {
        Self {
            parse,
            def_map,
            semantic_ir,
            materialization: None,
            packages,
            copy_compact_packages: normalized_package_slots(copy_compact_packages),
            interners,
            def_map_loader,
            semantic_ir_loader,
            subset,
            worker_limit: None,
            cancellation: rg_std::CancellationToken::new(),
        }
    }

    /// Select the bodies that [`BodyIrBuildPolicy`] allows for each package and Cargo target.
    pub fn configured_bodies(mut self, policy: BodyIrBuildPolicy) -> Self {
        self.materialization = Some(BodyIrMaterializationPlan::ConfiguredBodies(policy));
        self
    }

    /// Record which crates need body analysis, without analyzing their bodies yet.
    ///
    /// This lets the project answer queries about declarations before analyzing function bodies.
    /// Returns one [`PackageBodies`](crate::PackageBodies) per selected package, with an empty
    /// [`CrateBodies`] entry for each Cargo target. Its
    /// [`CrateBodiesCoverage`](crate::CrateBodiesCoverage) is `Missing` when body work remains,
    /// `SkippedByPolicy` for excluded targets, or `Complete` for included targets with no bodies.
    #[rg_std::cancelable("prepare body coverage")]
    pub fn prepare_coverage(
        self,
        policy: BodyIrBuildPolicy,
    ) -> anyhow::Result<Vec<(PackageSlot, PackageBodies)>> {
        let semantic_ir = self
            .semantic_ir
            .read_txn_for_subset(self.semantic_ir_loader, self.subset);
        let def_map = self
            .def_map
            .read_txn_for_subset(self.def_map_loader, self.subset);
        let packages = normalized_package_slots(self.packages);
        let lowered = lower::build_selected_packages(
            self.parse,
            &def_map,
            &semantic_ir,
            materialization::BodyIrMaterialization::CoverageOnly(policy),
            &packages,
            self.interners,
            self.worker_limit,
            None,
            &self.cancellation,
        )
        .context("prepare initial body coverage")?;
        Ok(lowered
            .into_iter()
            .map(|(package, crates)| {
                (
                    package,
                    PackageBodies::new(
                        crates
                            .into_iter()
                            .map(|(_, bodies)| CrateBodies::empty(bodies.coverage()))
                            .collect(),
                    ),
                )
            })
            .collect())
    }

    /// Select files and entire crates for the next build, including targets omitted by build policy.
    ///
    /// A crate in `crates` is built in full even if some of its files also appear in `files`.
    /// A file selection rebuilds only those files; to keep an earlier file's bodies, include it
    /// again. Other targets in the same package are left out of the results.
    pub fn selected_bodies(mut self, files: Vec<BodyIrFile>, crates: UniqueVec<CrateRef>) -> Self {
        self.materialization = Some(BodyIrMaterializationPlan::Selected { files, crates });
        self
    }

    /// Limit how many packages can be analyzed in parallel.
    ///
    /// `None` uses Rayon's default thread count. A smaller limit reduces the number of packages'
    /// temporary analysis data held in memory at the same time.
    pub fn worker_limit(mut self, worker_limit: Option<NonZeroUsize>) -> Self {
        self.worker_limit = worker_limit;
        self
    }

    /// Bind this build and its package workers to the operation that requested it.
    pub fn cancellation(mut self, cancellation: rg_std::CancellationToken) -> Self {
        self.cancellation = cancellation;
        self
    }

    /// Analyze the selected bodies and return their [`CrateBodies`] values with [`CrateRef`]s.
    ///
    /// Choose work with [`Self::configured_bodies`] or [`Self::selected_bodies`] first. Results
    /// are returned together after success; the caller then installs them in its body database.
    pub fn build(self) -> anyhow::Result<Vec<(CrateRef, CrateBodies)>> {
        let products = Mutex::new(Vec::new());
        self.build_with_package_priority(
            &|| Vec::new(),
            &|batch| {
                products
                    .lock()
                    .expect("body products should not be poisoned")
                    .extend(batch);
            },
            &|_| {},
        )
        .context("build body products")?;
        let mut products = products
            .into_inner()
            .expect("body products should not be poisoned");
        products.sort_by_key(|(crate_ref, _)| (crate_ref.package.0, crate_ref.crate_id.0));
        Ok(products)
    }

    /// Send each package's finished body results to `publish` as soon as they are ready.
    /// This lets callers use one package while other packages are still being analyzed.
    ///
    /// `publish` owns each batch and may be called concurrently by package workers.
    /// `priority_packages` supplies preferred packages between jobs; it cannot reorder work
    /// already running. The return value reports completion only. On error, all started workers
    /// have stopped, and batches already delivered remain with the caller.
    #[rg_std::cancelable("start body build")]
    pub fn build_with_package_priority(
        self,
        priority_packages: &(dyn Fn() -> Vec<PackageSlot> + Sync),
        publish: &(dyn Fn(Vec<(CrateRef, CrateBodies)>) + Sync),
        report_progress: &(dyn Fn(BodyIrBuildProgress) + Sync),
    ) -> anyhow::Result<()> {
        let packages = normalized_package_slots(self.packages);
        let materialization = self
            .materialization
            .as_ref()
            .context("body build requires a materialization selection")?
            .lowering();
        // Dependency payloads and solver caches are scoped to this build, even when the saved
        // declarations are offloaded. No decoded dependency escapes with a body product.
        let semantic_ir = self
            .semantic_ir
            .read_txn_for_subset(self.semantic_ir_loader, self.subset);
        let def_map = self
            .def_map
            .read_txn_for_subset(self.def_map_loader, self.subset);
        report_progress(BodyIrBuildProgress::new(
            BodyIrBuildStage::Lowering,
            0,
            packages.len(),
        ));
        let lowered = lower::build_selected_packages(
            self.parse,
            &def_map,
            &semantic_ir,
            materialization,
            &packages,
            self.interners,
            self.worker_limit,
            Some(report_progress),
            &self.cancellation,
        )
        .context("lower body products")?;
        report_progress(BodyIrBuildProgress::new(
            BodyIrBuildStage::Resolving,
            0,
            packages.len(),
        ));
        resolve::resolve_selected_packages(
            lowered,
            self.parse,
            self.interners,
            &def_map,
            &semantic_ir,
            priority_packages,
            &|mut batch| {
                // Copy-compaction is worthwhile for retained payloads; those headed to an artifact
                // can be encoded and dropped without a second allocation at the build's peak.
                if batch.first().is_some_and(|(crate_ref, _)| {
                    self.copy_compact_packages
                        .binary_search(&crate_ref.package)
                        .is_ok()
                }) {
                    batch = batch
                        .into_iter()
                        .map(|(crate_ref, bodies)| {
                            let mut compact = bodies.clone();
                            Shrink::shrink_to_fit(&mut compact);
                            (crate_ref, compact)
                        })
                        .collect();
                }
                publish(batch);
            },
            self.worker_limit,
            Some(report_progress),
            &self.cancellation,
        )
        .context("resolve body products")?;
        rg_std::check_cancel!(self.cancellation, "finish body build");
        Ok(())
    }
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
    use std::{
        fmt::Write as _,
        num::NonZeroUsize,
        sync::{
            Barrier,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use rg_def_map::{DefMapLoader, PackageSlot};
    use rg_package_store::PackageSubset;
    use rg_semantic_ir::SemanticIrLoader;
    use rg_std::CancellationToken;
    use rg_text::PackageNameInterners;

    use super::{BodyIrBuildProgress, BodyIrBuildStage, BodyIrBuilder, local_thread_pool};
    use crate::{BodyIrBuildPolicy, testonly::BodyIrFixture};

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

    #[test]
    fn cancelled_parallel_build_drains_started_workers_before_returning() {
        let mut source = String::from(
            "//- /Cargo.toml\n[workspace]\nmembers = [\"first\", \"second\", \"third\", \"fourth\"]\nresolver = \"3\"\n",
        );
        for name in ["first", "second", "third", "fourth"] {
            writeln!(
                source,
                r#"
//- /{name}/Cargo.toml
[package]
name = "{name}"
version = "0.1.0"
edition = "2024"

//- /{name}/src/lib.rs
pub fn compute() -> usize {{ let first = 1; first + 2 }}
"#
            )
            .expect("fixture text can be written");
        }
        let fixture = BodyIrFixture::build(&source);
        let packages = (0..fixture.parse_db().package_count())
            .map(PackageSlot)
            .collect::<Vec<_>>();
        let subset = PackageSubset::all(packages.len());
        let limit = NonZeroUsize::new(2).expect("two workers");
        let worker_count = local_thread_pool("cancellation-test", Some(limit))
            .expect("pool builds")
            .current_num_threads();
        for priority in [false, true] {
            let cancellation = CancellationToken::new();
            let barrier = Barrier::new(worker_count);
            let reached = AtomicUsize::new(0);
            let exited = AtomicUsize::new(0);
            let progress = |progress: BodyIrBuildProgress| {
                if progress.stage() == BodyIrBuildStage::Resolving
                    && progress.completed_packages() > 0
                {
                    reached.fetch_add(1, Ordering::SeqCst);
                    // All available workers are inside this build before any of them cancels it.
                    barrier.wait();
                    cancellation.cancel();
                    exited.fetch_add(1, Ordering::SeqCst);
                }
            };
            let mut names = PackageNameInterners::new(packages.len());
            let builder = BodyIrBuilder::new(
                fixture.parse_db(),
                fixture.def_map_db(),
                fixture.semantic_ir_db(),
                &packages,
                &packages,
                &mut names,
                DefMapLoader::resident_only("parallel fixture"),
                SemanticIrLoader::resident_only("parallel fixture"),
                &subset,
            )
            .configured_bodies(BodyIrBuildPolicy::default())
            .worker_limit(Some(limit))
            .cancellation(cancellation.clone());
            let priorities = || {
                if priority {
                    vec![PackageSlot(0)]
                } else {
                    Vec::new()
                }
            };
            let delivered = AtomicUsize::new(0);
            let result = builder.build_with_package_priority(
                &priorities,
                &|products| {
                    delivered.fetch_add(products.len(), Ordering::SeqCst);
                },
                &progress,
            );
            let error =
                result.expect_err("cancelled worker set reports cancellation after draining");
            assert!(error.chain().any(|cause| cause.is::<rg_std::Cancelled>()));
            assert_eq!(reached.load(Ordering::SeqCst), worker_count);
            assert_eq!(exited.load(Ordering::SeqCst), worker_count);
            assert_eq!(
                delivered.load(Ordering::SeqCst),
                worker_count,
                "only completed jobs deliver their products before the cancellation barrier"
            );
        }
        let mut names = PackageNameInterners::new(packages.len());
        BodyIrBuilder::new(
            fixture.parse_db(),
            fixture.def_map_db(),
            fixture.semantic_ir_db(),
            &packages,
            &packages,
            &mut names,
            DefMapLoader::resident_only("parallel retry"),
            SemanticIrLoader::resident_only("parallel retry"),
            &subset,
        )
        .configured_bodies(BodyIrBuildPolicy::default())
        .worker_limit(Some(limit))
        .build()
        .expect("fresh build succeeds after workers joined");
    }
}
