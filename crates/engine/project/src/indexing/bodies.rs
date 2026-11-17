//! Completes whole targets that the eager build policy intentionally skipped.

use anyhow::Context as _;
use rg_body_ir::{BodyIrBuilder, CrateBodiesCoverage, PackageBodiesCoverage};
use rg_ir_model::CrateRef;
use rg_package_store::PackageStoreError;
use rg_std::UniqueVec;

use crate::{
    Project,
    selection::PhasePackageSet,
    storage::{
        cache::{BodyIrWriteInput, PackageCacheBodyUpdateInput},
        loaders::PackageReadLoaders,
    },
};

impl Project {
    /// Make saved bodies available for a query into targets excluded by the startup policy.
    ///
    /// Startup finishes primary workspace targets. A query can also enter a test target or a
    /// dependency. Complete that whole target synchronously, keeping other targets unchanged.
    /// Exclusive project access means no save or competing body build can intervene.
    /// TODO: replace this whole-target fallback with Ajika body queries after storage migration.
    pub fn ensure_body_analysis(
        &mut self,
        crates: &[CrateRef],
        cancellation: &rg_std::CancellationToken,
    ) -> anyhow::Result<()> {
        let state = &mut self.state;
        let crates = crates
            .iter()
            .copied()
            .filter(|&crate_ref| {
                !state
                    .body_ir
                    .crate_coverage(crate_ref)
                    .is_some_and(CrateBodiesCoverage::is_complete)
            })
            .collect::<UniqueVec<_>>();
        if crates.is_empty() {
            return Ok(());
        }

        let packages = PhasePackageSet::from_crates(crates.as_slice());
        let subset = packages.visible_dependency_subset(&state.workspace);
        let loaders = PackageReadLoaders::new(state);
        let products = BodyIrBuilder::new(
            &state.parse,
            &state.def_map,
            &state.semantic_ir,
            packages.as_slice(),
            !state.package_residency_policy.is_offloadable(),
            loaders.def_map,
            loaders.semantic_ir,
            &subset,
        )
        .cancellation(cancellation.clone())
        .selected_crates(crates)
        .build();
        state.parse.evict_saved_source_text();
        let mut products = products?.into_iter().peekable();

        // The builder returns results in package/target order. Replace complete targets together;
        // there is no partial-file merge or independently published background result.
        while let Some((first, _)) = products.peek() {
            rg_std::check_cancel!(cancellation, "complete saved body targets");
            let package = first.package;
            let mut replacements = Vec::new();
            while products
                .peek()
                .is_some_and(|(crate_ref, _)| crate_ref.package == package)
            {
                let (crate_ref, bodies) = products.next().expect("peeked body product exists");
                replacements.push((crate_ref.crate_id, bodies));
            }

            if let Some(current) = state.body_ir.resident_package(package) {
                let mut next = current.clone();
                for (crate_id, bodies) in replacements {
                    next.replace_crate(crate_id, bodies)
                        .context("body target slot must exist")?;
                }
                state
                    .body_ir
                    .replace_package(package, next)
                    .expect("existing body package");
            } else {
                // Copy unrelated bodies and declarations from the existing artifact without
                // loading them into the query working set.
                let header = state
                    .cache_plan
                    .artifact_header(package, &state.package_source_fingerprints)
                    .context("body package source identity must exist")?;
                let reader = state
                    .cache_store
                    .open_artifact(&header)
                    .map_err(|error| error.into_package_store_error(package))?
                    .ok_or_else(|| PackageStoreError::missing_package(package))?;
                let manifest = reader
                    .read_body_ir_manifest()
                    .map_err(|error| error.into_package_store_error(package))?;
                let body_ir = BodyIrWriteInput::update(&manifest, &replacements);
                let coverage = PackageBodiesCoverage::from_crates(body_ir.coverage());
                let parse = state
                    .parse
                    .package(package.0)
                    .expect("existing body package")
                    .parse_snapshot()?;
                let artifact = state.cache_store.prepare_body_update(
                    package,
                    PackageCacheBodyUpdateInput {
                        header: &header,
                        parse: &parse,
                        body_ir: &body_ir,
                    },
                    &reader,
                    cancellation,
                )?;
                rg_std::check_cancel!(cancellation, "commit completed body targets");
                // The file and its directory must change together, without a cancellation point
                // after commit. No source generation changes: these bodies use the saved inputs.
                artifact.commit()?;
                state
                    .body_ir
                    .replace_offloaded_package(package, coverage)
                    .expect("existing offloaded body package");
            }
        }
        Ok(())
    }
}
