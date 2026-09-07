//! Adds independently built body analysis to the saved project.
//!
//! A worker may finish after a save or after another request analyzed some of the same files.
//! Check its saved project version and analyzed file set before accepting its results. If a cache
//! write is needed, prepare the bytes first, then replace the file and update the project while
//! holding exclusive mutable access.

use super::{SavedBodyProducts, configured_package_is_finished};
use crate::{
    PackageResidency,
    cache::{BodyIrWriteInput, PackageCacheBodyUpdateInput, PackageCacheWriteInput},
    project::state::ProjectState,
};
use anyhow::Context as _;
use rg_body_ir::{CrateBodies, PackageBodiesCoverage};
use rg_def_map::PackageSlot;
use rg_ir_model::{CrateId, CrateRef};
use rg_package_store::PackageStoreError;
use rg_std::Shrink;
use std::sync::Arc;

/// How [`crate::SplitIndexing::publish`] handled one crate's body analysis results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyPublicationOutcome {
    /// The crate's new body analysis was installed in the project.
    Installed,
    /// Existing analysis already covers every file in this result. The existing data and its
    /// storage location are kept.
    AlreadyCovered,
    /// The build used an older saved project version. Its results were discarded before cache access.
    ObsoleteGeneration,
    /// The result would lose some already-analyzed files, or its package moved to disk while a
    /// partial build ran. Call [`crate::SplitIndexing::prepare`] again to include existing work
    /// or request a full target suitable for caching.
    ReplanRequired,
}

/// Lists what [`crate::SplitIndexing::publish`] did with each crate in a batch of results.
/// Some crates may be installed while others keep their existing analysis or need another build.
#[derive(Debug)]
pub struct BodyPublication {
    outcomes: Vec<(CrateRef, BodyPublicationOutcome)>,
}

impl BodyPublication {
    pub fn outcomes(&self) -> &[(CrateRef, BodyPublicationOutcome)] {
        &self.outcomes
    }
    pub fn improved(&self) -> bool {
        self.outcomes
            .iter()
            .any(|(_, outcome)| *outcome == BodyPublicationOutcome::Installed)
    }
}

pub(super) fn publish(
    state: &mut ProjectState,
    mut products: SavedBodyProducts,
    cancellation: &rg_std::CancellationToken,
) -> anyhow::Result<BodyPublication> {
    // Check the stamp here, before even opening an artifact. Direct library callers receive the
    // same stale-result protection as the LSP; its separate lifecycle counter is not authority.
    if products.generation != state.generation_id {
        return Ok(BodyPublication {
            outcomes: products
                .crates
                .into_iter()
                .map(|(crate_ref, _)| (crate_ref, BodyPublicationOutcome::ObsoleteGeneration))
                .collect(),
        });
    }
    products
        .crates
        .sort_by_key(|(crate_ref, _)| (crate_ref.package.0, crate_ref.crate_id.0));
    let mut products = products.crates.into_iter().peekable();
    let mut outcomes = Vec::new();
    while let Some((first, _)) = products.peek() {
        rg_std::check_cancel!(cancellation, "prepare body publication");
        let package = first.package;
        let offloaded = state.body_ir.package_is_offloaded(package);
        let mut replacements = Vec::new();
        while products
            .peek()
            .is_some_and(|(crate_ref, _)| crate_ref.package == package)
        {
            let (crate_ref, bodies) = products.next().expect("peeked product should exist");
            let current = state
                .body_ir
                .crate_coverage(crate_ref)
                .context("body product must address an existing crate")?;
            let incoming = bodies.coverage();
            let outcome = if current.contains(incoming) {
                BodyPublicationOutcome::AlreadyCovered
            } else if !incoming.contains(current) || (offloaded && !incoming.is_complete()) {
                BodyPublicationOutcome::ReplanRequired
            } else {
                replacements.push((crate_ref.crate_id, bodies));
                BodyPublicationOutcome::Installed
            };
            outcomes.push((crate_ref, outcome));
        }
        if replacements.is_empty() {
            continue;
        }
        publish_package(state, package, replacements, cancellation)
            .context("publish body package")?;
    }
    rg_std::check_cancel!(cancellation, "after body publication");
    Ok(BodyPublication { outcomes })
}

fn publish_package(
    state: &mut ProjectState,
    package: PackageSlot,
    replacements: Vec<(CrateId, CrateBodies)>,
    cancellation: &rg_std::CancellationToken,
) -> anyhow::Result<()> {
    if let Some(current) = state.body_ir.resident_package(package) {
        // Clone the list of crate handles, then replace only the requested entries. Other targets
        // and existing readers keep their body data, facts and local declarations together.
        let mut next = current.clone();
        for (crate_id, bodies) in replacements {
            next.replace_crate(crate_id, bodies)
                .context("body product crate slot must exist")?;
        }
        let offload = state.package_residency.package(package)
            == Some(PackageResidency::Offloadable)
            && configured_package_is_finished(state, package, &next);
        let artifact = if offload {
            let header = state
                .cache_plan
                .artifact_header(package, &state.package_source_fingerprints)
                .context("prepare body artifact identity")?;
            let parse = state
                .parse
                .package(package.0)
                .context("body package parse data must exist")?
                .parse_snapshot()
                .context("snapshot body package source")?;
            let def_map = state
                .def_map
                .resident_package(package)
                .context("resident body package needs resident declarations")?;
            let semantic_ir = state
                .semantic_ir
                .resident_package(package)
                .context("resident body package needs resident semantic IR")?;
            Some(
                state
                    .cache_store
                    .prepare_write_input(
                        PackageCacheWriteInput::new(&header, &parse, def_map, semantic_ir, &next),
                        cancellation,
                    )
                    .context("stage complete body artifact")?,
            )
        } else {
            None
        };
        #[cfg(test)]
        crate::tests::cancellation::materialization_checkpoint(
            crate::tests::cancellation::MaterializationPoint::BeforePublication,
            cancellation,
        );
        rg_std::check_cancel!(cancellation, "commit resident body products");
        // No cancellation between artifact replacement and the matching in-memory transition.
        // Every slot below was validated before preparation; no fallible work follows the commit.
        if let Some(artifact) = artifact {
            artifact.commit().context("commit body artifact")?;
        }
        state
            .body_ir
            .replace_package(package, next)
            .expect("validated body package must exist");
        if offload {
            state
                .def_map
                .offload_package(package)
                .expect("validated DefMap package must exist");
            state
                .semantic_ir
                .offload_package(package)
                .expect("validated semantic package must exist");
            state
                .body_ir
                .offload_package(package)
                .expect("validated body package must exist");
            Shrink::shrink_to_fit(&mut state.names);
            Arc::make_mut(&mut state.parse).offload_line_indexes_for_packages(&[package.0]);
        }
    } else {
        // Read unchanged targets' manifests from the latest cache file: another body build may
        // have finished since these inputs were captured. Reuse this reader for the declarations
        // and copied bodies too, so all bytes come from the same version of the file.
        let header = state
            .cache_plan
            .artifact_header(package, &state.package_source_fingerprints)
            .context("prepare cached body artifact identity")?;
        let reader = state
            .cache_store
            .open_artifact(&header)
            .map_err(|error| error.into_package_store_error(package))
            .context("open current body artifact")?
            .ok_or_else(|| PackageStoreError::missing_package(package))
            .context("offloaded body artifact must exist")?;
        let manifest = reader
            .read_body_ir_manifest()
            .map_err(|error| error.into_package_store_error(package))
            .context("read current body manifest")?;
        let body_ir = BodyIrWriteInput::update(&manifest, &replacements);
        let coverage = PackageBodiesCoverage::from_crates(body_ir.coverage());
        let parse = state
            .parse
            .package(package.0)
            .context("cached body package parse data must exist")?
            .parse_snapshot()
            .context("snapshot cached body package source")?;
        let artifact = state
            .cache_store
            .prepare_body_update(
                package,
                PackageCacheBodyUpdateInput {
                    header: &header,
                    parse: &parse,
                    body_ir: &body_ir,
                },
                &reader,
                cancellation,
            )
            .context("stage cached body replacements")?;
        #[cfg(test)]
        crate::tests::cancellation::materialization_checkpoint(
            crate::tests::cancellation::MaterializationPoint::BeforePublication,
            cancellation,
        );
        rg_std::check_cancel!(cancellation, "commit cached body products");
        artifact.commit().context("commit cached body artifact")?;
        state
            .body_ir
            .replace_offloaded_package(package, coverage)
            .expect("validated offloaded package must exist");
    }
    Ok(())
}
