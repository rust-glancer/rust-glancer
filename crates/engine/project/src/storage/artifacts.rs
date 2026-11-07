//! Keeps package cache artifacts coherent across their phase databases.
//!
//! One package artifact stores Parse metadata, DefMap, Semantic IR, and Body IR together. All
//! artifact-backed phase payloads must be written before they are offloaded, and all three phase
//! databases must be offloaded as one lifecycle step.

use anyhow::Context as _;
use rayon::prelude::*;
use rg_body_ir::BodyIrDb;
use rg_def_map::DefMapDb;
use rg_ir_model::PackageSlot;
use rg_parse::ParseDb;
use rg_semantic_ir::SemanticIrDb;
use rg_std::CancellationToken;

use crate::{
    state::ProjectState,
    storage::cache::{
        Fingerprint, PackageCacheStore, PackageCacheWriteInput, PreparedPackageArtifact,
        WorkspaceCachePlan,
    },
};

/// Mutable view of the phase databases backed by one package artifact.
///
/// Offloading through this view prevents one call site from forgetting a phase when the artifact
/// format gains or changes a package-backed payload.
pub(crate) struct PackageArtifactPhases<'a> {
    def_map: &'a mut DefMapDb,
    semantic_ir: &'a mut SemanticIrDb,
    body_ir: &'a mut BodyIrDb,
}

impl<'a> PackageArtifactPhases<'a> {
    pub(crate) fn for_project(project: &'a mut ProjectState) -> Self {
        Self {
            def_map: &mut project.def_map,
            semantic_ir: &mut project.semantic_ir,
            body_ir: &mut project.body_ir,
        }
    }

    /// Offloads one package from every artifact-backed phase database.
    pub(crate) fn offload_package(&mut self, package: PackageSlot) -> anyhow::Result<()> {
        // Only drop resident data after the full cross-phase package artifact is durable. If a
        // future implementation downgrades write errors to warnings, this invariant should remain.
        self.def_map.offload_package(package).with_context(|| {
            format!("while attempting to offload def-map package {}", package.0)
        })?;
        self.semantic_ir.offload_package(package).with_context(|| {
            format!(
                "while attempting to offload semantic IR package {}",
                package.0
            )
        })?;
        self.body_ir.offload_package(package).with_context(|| {
            format!("while attempting to offload body IR package {}", package.0)
        })?;

        Ok(())
    }
}

/// Encodes coherent package artifacts from a borrowed set of resident phase databases.
///
/// All three phases and the parse snapshot are borrowed from one generation.
pub(crate) struct PackageArtifactWriter<'a> {
    cache_store: &'a PackageCacheStore,
    cache_plan: &'a WorkspaceCachePlan,
    package_source_fingerprints: &'a [Option<Fingerprint>],
    parse: &'a ParseDb,
    def_map: &'a DefMapDb,
    semantic_ir: &'a SemanticIrDb,
    body_ir: &'a BodyIrDb,
}

impl<'a> PackageArtifactWriter<'a> {
    pub(crate) fn for_project(project: &'a ProjectState) -> Self {
        Self {
            cache_store: &project.cache_store,
            cache_plan: &project.cache_plan,
            package_source_fingerprints: &project.package_source_fingerprints,
            parse: &project.parse,
            def_map: &project.def_map,
            semantic_ir: &project.semantic_ir,
            body_ir: &project.body_ir,
        }
    }

    /// Writes temporary artifacts so the owner can check freshness before replacing them.
    pub(crate) fn prepare_packages(
        &self,
        packages: &[PackageSlot],
    ) -> anyhow::Result<Vec<PreparedPackageArtifact>> {
        if packages.is_empty() {
            return Ok(Vec::new());
        }
        let thread_pool = Self::local_thread_pool("rg-cache-write")
            .context("while attempting to prepare package cache writers")?;

        thread_pool.install(|| {
            packages
                .par_iter()
                .map(|&package| self.prepare_package(package))
                .collect()
        })
    }

    /// Writes one coherent package artifact from the phase data available at this boundary.
    ///
    /// Fresh construction and saved-source updates supply all three resident phases from the
    /// same private candidate. Body-only publication prepares its artifact separately.
    fn prepare_package(&self, package: PackageSlot) -> anyhow::Result<PreparedPackageArtifact> {
        let header = self
            .cache_plan
            .artifact_header(package, self.package_source_fingerprints)
            .with_context(|| {
                format!(
                    "while attempting to build package cache header for package {}",
                    package.0,
                )
            })?;
        let parse = self.parse.package(package.0).with_context(|| {
            format!(
                "while attempting to fetch parsed package {} for cache artifact",
                package.0,
            )
        })?;
        let body_ir = self.body_ir.resident_package(package).with_context(|| {
            format!(
                "while attempting to fetch resident body IR package {}",
                package.0,
            )
        })?;

        let parse = parse.parse_snapshot();

        let def_map = self
            .def_map
            .resident_package(package)
            .context("artifact needs resident declarations")?;
        let semantic_ir = self
            .semantic_ir
            .resident_package(package)
            .context("artifact needs resident semantic IR")?;
        let input = PackageCacheWriteInput::new(&header, &parse, def_map, semantic_ir, body_ir);
        self.cache_store
            .prepare_write_input(input, &CancellationToken::new())
            .with_context(|| format!("prepare cache artifact for package {}", package.0))
    }

    /// Creates a short-lived Rayon pool for package artifact serialization.
    fn local_thread_pool(thread_name_prefix: &'static str) -> anyhow::Result<rayon::ThreadPool> {
        rayon::ThreadPoolBuilder::new()
            .thread_name(move |index| format!("{thread_name_prefix}-{index}"))
            .build()
            .with_context(|| format!("while attempting to create {thread_name_prefix} thread pool"))
    }
}
