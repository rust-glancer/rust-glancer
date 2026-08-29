//! Keeps package cache artifacts coherent across their phase databases.
//!
//! One package artifact stores Parse metadata, DefMap, Semantic IR, and Body IR together. A caller
//! may write that artifact during a normal residency change or while batch indexing is still
//! assembling the project. In both cases, all artifact-backed phase payloads must be written before
//! they are offloaded, and all three phase databases must be offloaded as one lifecycle step.

use anyhow::Context as _;
use rayon::prelude::*;
use rg_body_ir::BodyIrDb;
use rg_def_map::{DefMapDb, PackageSlot};
use rg_parse::ParseDb;
use rg_semantic_ir::SemanticIrDb;

use crate::cache::{
    Fingerprint, PackageCacheBodyUpdateInput, PackageCacheStore, PackageCacheUpdate,
    PackageCacheWriteInput, WorkspaceCachePlan,
};

use super::state::ProjectState;

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
        Self::new(
            &mut project.def_map,
            &mut project.semantic_ir,
            &mut project.body_ir,
        )
    }

    pub(crate) fn new(
        def_map: &'a mut DefMapDb,
        semantic_ir: &'a mut SemanticIrDb,
        body_ir: &'a mut BodyIrDb,
    ) -> Self {
        Self {
            def_map,
            semantic_ir,
            body_ir,
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
/// Ordinary residency owns a complete [`ProjectState`], while batch indexing needs to persist one
/// finished package group before that state has been assembled. This view keeps both lifecycle
/// paths on the same artifact-writing protocol: all three phases and the parse snapshot are borrowed
/// from one generation and written through a caller-owned cache transaction.
pub(crate) struct PackageArtifactWriter<'a> {
    cache_plan: &'a WorkspaceCachePlan,
    cache_store: &'a PackageCacheStore,
    package_source_fingerprints: &'a [Option<Fingerprint>],
    parse: &'a ParseDb,
    def_map: &'a DefMapDb,
    semantic_ir: &'a SemanticIrDb,
    body_ir: &'a BodyIrDb,
}

impl<'a> PackageArtifactWriter<'a> {
    pub(crate) fn for_project(project: &'a ProjectState) -> Self {
        Self::new(
            &project.cache_plan,
            &project.cache_store,
            &project.package_source_fingerprints,
            &project.parse,
            &project.def_map,
            &project.semantic_ir,
            &project.body_ir,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        cache_plan: &'a WorkspaceCachePlan,
        cache_store: &'a PackageCacheStore,
        package_source_fingerprints: &'a [Option<Fingerprint>],
        parse: &'a ParseDb,
        def_map: &'a DefMapDb,
        semantic_ir: &'a SemanticIrDb,
        body_ir: &'a BodyIrDb,
    ) -> Self {
        Self {
            cache_plan,
            cache_store,
            package_source_fingerprints,
            parse,
            def_map,
            semantic_ir,
            body_ir,
        }
    }

    /// Writes package-local artifacts without committing the surrounding package-set update.
    ///
    /// Batch indexing deliberately keeps one transaction open while later batches read artifacts
    /// written by earlier batches. The owner commits only after every selected package succeeds,
    /// so a failed build still leaves the cache's incomplete-update marker intact.
    pub(crate) fn write_packages(
        &self,
        update: &PackageCacheUpdate<'_>,
        packages: &[PackageSlot],
    ) -> anyhow::Result<()> {
        let thread_pool = Self::local_thread_pool("rg-cache-write")
            .context("while attempting to prepare package cache writers")?;

        // Artifact serialization is package-local and usually more expensive than the final state
        // mutation. Write every durable artifact first; only then can callers safely drop residents.
        thread_pool.install(|| {
            packages
                .par_iter()
                .try_for_each(|package| self.write_package(update, *package))
        })
    }

    /// Writes one coherent package artifact from the phase data available at this boundary.
    ///
    /// Jointly resident declaration phases are encoded normally and may reuse untouched Body shards.
    /// If both declaration phases are offloaded, an exact Body rebuild instead copies their encoded
    /// sections from the pinned prior revision. A mixed declaration residency state is rejected.
    fn write_package(
        &self,
        update: &PackageCacheUpdate<'_>,
        package: PackageSlot,
    ) -> anyhow::Result<()> {
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

        let parse = parse.parse_snapshot().with_context(|| {
            format!(
                "while attempting to snapshot parse metadata for package {}",
                package.0,
            )
        })?;

        let def_map = self.def_map.resident_package(package);
        let semantic_ir = self.semantic_ir.resident_package(package);
        let write = match (def_map, semantic_ir) {
            (Some(def_map), Some(semantic_ir)) => {
                let input =
                    PackageCacheWriteInput::new(&header, &parse, def_map, semantic_ir, body_ir);
                if body_ir.has_cached_payloads() {
                    let reader = self
                        .cache_store
                        .open_artifact(&header)
                        .with_context(|| {
                            format!(
                                "while attempting to open prior cache artifact for package {}",
                                package.0,
                            )
                        })?
                        .with_context(|| {
                            format!(
                                "prior cache artifact is missing for package {} with cached Body IR payloads",
                                package.0,
                            )
                        })?;
                    update.write_input_reusing_cached_body_ir(input, &reader)
                } else {
                    update.write_input(input)
                }
            }
            (None, None) => {
                // Exact target materialization starts from a fully offloaded artifact. It restores
                // a manifest-backed Body IR package and rebuilds only the requested target;
                // declaration reads belong to short-lived transactions, so DefMap and Semantic IR
                // remain offloaded. Cached Body placeholders distinguish that rewrite overlay from
                // an arbitrary mixed-residency state.
                anyhow::ensure!(
                    self.def_map.package_is_offloaded(package)
                        && self.semantic_ir.package_is_offloaded(package),
                    "package {} declaration phases are neither jointly resident nor offloaded",
                    package.0,
                );
                anyhow::ensure!(
                    body_ir.has_cached_payloads(),
                    "package {} needs cached declaration sections without cached Body IR siblings",
                    package.0,
                );

                // Pin the old revision before the atomic replacement. The changed Body IR is
                // encoded normally; sibling Body shards and both declaration sections are copied.
                let input = PackageCacheBodyUpdateInput::new(&header, &parse, body_ir);
                let reader = self
                    .cache_store
                    .open_artifact(&header)
                    .with_context(|| {
                        format!(
                            "while attempting to open prior cache artifact for package {}",
                            package.0,
                        )
                    })?
                    .with_context(|| {
                        format!(
                            "prior cache artifact is missing for package {} with cached declarations",
                            package.0,
                        )
                    })?;
                update.write_body_update_reusing_cached_sections(input, &reader)
            }
            _ => anyhow::bail!(
                "package {} has inconsistent DefMap and Semantic IR residency",
                package.0,
            ),
        };

        write.with_context(|| {
            format!(
                "while attempting to write package cache artifact for package {}",
                package.0,
            )
        })
    }

    /// Creates a short-lived Rayon pool for package artifact serialization.
    fn local_thread_pool(thread_name_prefix: &'static str) -> anyhow::Result<rayon::ThreadPool> {
        rayon::ThreadPoolBuilder::new()
            .thread_name(move |index| format!("{thread_name_prefix}-{index}"))
            .build()
            .with_context(|| format!("while attempting to create {thread_name_prefix} thread pool"))
    }
}
