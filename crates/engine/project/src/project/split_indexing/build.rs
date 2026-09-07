//! Captures saved source and declarations so body analysis can run outside the live project.

use super::{AnalysisSurface, SplitIndexingProgress, unfinished_crates};
use crate::project::{
    loading::PackageReadLoaders,
    package_set::PhasePackageSet,
    state::{ProjectGenerationId, ProjectState},
};
use anyhow::Context as _;
use rg_body_ir::{BodyIrBuilder, BodyIrFile, CrateBodies, CrateBodiesCoverage};
use rg_def_map::{DefMapDb, DefMapLoader, PackageSlot};
use rg_ir_model::CrateRef;
use rg_package_store::PackageSubset;
use rg_parse::ParseDb;
use rg_semantic_ir::{SemanticIrDb, SemanticIrLoader};
use rg_std::{MemorySize, UniqueVec};
use rg_text::PackageNameInterners;
use std::{
    num::NonZeroUsize,
    path::Path,
    sync::{Arc, Mutex},
};

/// Owned inputs for analyzing selected bodies from one saved version of [`Project`](crate::Project).
///
/// Created by [`Project::deferred_body_build`](crate::Project::deferred_body_build) for background
/// work, or [`SplitIndexing::prepare`](crate::SplitIndexing::prepare) for a query. The source and
/// declarations are shared with that project version, so these inputs can move to a worker while
/// the live project accepts newer changes. Building returns [`SavedBodyProducts`]; installing
/// them is a separate [`SplitIndexing::publish`](crate::SplitIndexing::publish) step.
#[derive(Debug)]
pub struct SavedBodyBuildInputs {
    generation: ProjectGenerationId,
    parse: Arc<ParseDb>,
    def_map: DefMapDb,
    semantic_ir: SemanticIrDb,
    def_map_loader: DefMapLoader<'static>,
    semantic_ir_loader: SemanticIrLoader<'static>,
    files: Vec<BodyIrFile>,
    crates: UniqueVec<CrateRef>,
    packages: Vec<PackageSlot>,
    subset: PackageSubset,
    compact_packages: Vec<PackageSlot>,
    worker_limit: Option<NonZeroUsize>,
}

/// Body analysis results ready to be installed in a [`Project`](crate::Project).
///
/// Each result pairs a [`CrateRef`] with its new [`CrateBodies`]. The batch also records the
/// [`ProjectGenerationId`] of the source and declarations used by the build. Pass it to
/// [`SplitIndexing::publish`](crate::SplitIndexing::publish), which rejects results from an older
/// project version and checks for work completed by other requests since the build started.
#[derive(Debug, MemorySize)]
pub struct SavedBodyProducts {
    pub(super) generation: ProjectGenerationId,
    pub(super) crates: Vec<(CrateRef, CrateBodies)>,
}

impl SavedBodyProducts {
    pub fn generation_id(&self) -> ProjectGenerationId {
        self.generation
    }
}

impl SavedBodyBuildInputs {
    pub(crate) fn deferred(state: &ProjectState) -> Self {
        let crates = unfinished_crates(state).collect::<Vec<_>>();
        Self::for_surface(state, AnalysisSurface::Crates(&crates))
    }

    /// Choose missing body work and include previously analyzed files in each requested crate.
    pub(super) fn for_surface(state: &ProjectState, surface: AnalysisSurface<'_>) -> Self {
        let (requested_files, requested_crates) = surface.parts();
        let mut crates = requested_crates
            .iter()
            .copied()
            .filter(|&crate_ref| {
                !state
                    .body_ir
                    .crate_coverage(crate_ref)
                    .is_some_and(CrateBodiesCoverage::is_complete)
            })
            .collect::<UniqueVec<_>>();
        let mut files = UniqueVec::new();
        for &(crate_ref, file) in requested_files {
            let coverage = state.body_ir.crate_coverage(crate_ref);
            if coverage.is_some_and(|coverage| coverage.contains_file(file)) {
                continue;
            }
            // Partial coverage is transient. An offloaded target is completed before rewriting its
            // package artifact, leaving unrelated targets encoded in that artifact.
            if state.body_ir.package_is_offloaded(crate_ref.package) {
                crates.push(crate_ref);
                continue;
            }
            files.push(BodyIrFile::new(crate_ref, file));
            // If first.rs was prepared before a query asks for second.rs, rebuild both together.
            // Their body ids must come from one crate revision. Coverage also keeps empty files
            // ready, even though scanning the old body arenas would not find them.
            if let Some(CrateBodiesCoverage::Files(processed)) = coverage {
                for &file in processed {
                    files.push(BodyIrFile::new(crate_ref, file));
                }
            }
        }
        let files = files
            .into_vec()
            .into_iter()
            .filter(|file| !crates.contains(&file.crate_ref))
            .collect::<Vec<_>>();
        let selected = files
            .iter()
            .map(|file| file.crate_ref)
            .chain(crates.iter().copied())
            .collect::<Vec<_>>();
        let packages = PhasePackageSet::from_crates(&selected);
        let subset = packages.visible_dependency_subset(&state.workspace);
        // File work may leave an offloadable package incomplete, so retain compact crate payloads
        // there too. Complete background products headed directly to disk do not need the copy.
        let compact_packages = if files.is_empty() {
            state
                .package_residency
                .resident_packages(packages.as_slice())
        } else {
            packages.as_slice().to_vec()
        };
        let loaders = PackageReadLoaders::new(state);
        Self {
            generation: state.generation_id,
            parse: Arc::clone(&state.parse),
            def_map: state.def_map.clone(),
            semantic_ir: state.semantic_ir.clone(),
            def_map_loader: loaders.def_map,
            semantic_ir_loader: loaders.semantic_ir,
            files,
            crates,
            packages: packages.as_slice().to_vec(),
            subset,
            compact_packages,
            worker_limit: state.indexing_preference.body_ir_worker_limit(),
        }
    }

    pub fn generation_id(&self) -> ProjectGenerationId {
        self.generation
    }
    pub fn packages(&self) -> &[PackageSlot] {
        &self.packages
    }

    pub fn package_slots_for_path(&self, path: &Path) -> anyhow::Result<Vec<PackageSlot>> {
        PhasePackageSet::from_path(&self.parse, path).map(PhasePackageSet::into_vec)
    }

    /// Analyze the selected bodies and return all results as [`SavedBodyProducts`].
    /// Submit them through [`SplitIndexing::publish`](crate::SplitIndexing::publish) after success.
    /// If construction fails, completed results are dropped and the live project is unchanged.
    pub fn build(
        self,
        cancellation: &rg_std::CancellationToken,
    ) -> anyhow::Result<SavedBodyProducts> {
        let generation = self.generation;
        let products = Mutex::new(Vec::new());
        self.build_with_package_priority(
            &|| Vec::new(),
            &|batch| {
                products
                    .lock()
                    .expect("body products should not be poisoned")
                    .extend(batch.crates);
            },
            &|_| {},
            cancellation,
        )
        .context("build saved bodies")?;
        Ok(SavedBodyProducts {
            generation,
            crates: products
                .into_inner()
                .expect("body products should not be poisoned"),
        })
    }

    /// Analyze the selected bodies, passing each package's results to `publish` when ready.
    /// The callback owns each [`SavedBodyProducts`] batch and can send it to the thread that
    /// owns the project for [`SplitIndexing::publish`](crate::SplitIndexing::publish).
    ///
    /// Callbacks may run concurrently. `priority` supplies preferred packages between jobs, so
    /// changes to it affect only work that has not started. This call returns completion status
    /// after all workers stop; an error does not take back batches already sent to `publish`.
    pub fn build_with_package_priority(
        self,
        priority: &(dyn Fn() -> Vec<PackageSlot> + Sync),
        publish: &(dyn Fn(SavedBodyProducts) + Sync),
        report: &(dyn Fn(SplitIndexingProgress) + Sync),
        cancellation: &rg_std::CancellationToken,
    ) -> anyhow::Result<()> {
        let mut names = PackageNameInterners::new(self.parse.package_count());
        let result = BodyIrBuilder::new(
            &self.parse,
            &self.def_map,
            &self.semantic_ir,
            &self.packages,
            &self.compact_packages,
            &mut names,
            self.def_map_loader,
            self.semantic_ir_loader,
            &self.subset,
        )
        .worker_limit(self.worker_limit)
        .cancellation(cancellation.clone())
        .selected_bodies(self.files, self.crates)
        .build_with_package_priority(
            priority,
            &|crates| {
                publish(SavedBodyProducts {
                    generation: self.generation,
                    crates,
                })
            },
            &|progress| report(SplitIndexingProgress::from_body_ir(progress)),
        );
        // Reloadable text and weak interner tables are build-scoped on both success and failure.
        // Names in a completed payload own their strings and need no interner publication.
        self.parse.evict_saved_source_text();
        result.context("construct saved body products")
    }
}
