//! Selects source work and retains the routing metadata accepted from startup cache probes.

use rg_body_ir::{BodyIrBuildPolicy, CrateBodiesCoverage, PackageBodiesCoverage};
use rg_ir_model::PackageSlot;
use rg_parse::ParseDb;
use rg_std::MemorySize;
use rg_workspace::WorkspaceMetadata;

use super::cache_probe::{StartupCacheProbe, StartupPackageSelection};
use crate::{
    PackageResidencyPlan, StartupCacheLoad,
    selection::PhasePackageSet,
    storage::cache::{PackageCacheStore, WorkspaceCachePlan},
};

/// Phase inputs retained after optional startup cache probing.
///
/// Packages omitted from `source_packages` already have matching offloaded artifacts, so later
/// build phases can read them lazily instead of lowering them from source. The compact DefMap
/// directories route dependency and file queries, while Body IR coverage tells its provisional
/// package-store entry which deferred data is already available.
#[derive(MemorySize)]
pub(super) struct PackageBuildPlan {
    pub(super) source_packages: PhasePackageSet,
    /// A [`rg_def_map::PackageDefMapsManifest`] for each cache hit, mapping files to crate payloads.
    /// Packages rebuilt from source have no valid cached manifest.
    pub(super) def_map_manifests: Vec<Option<rg_def_map::PackageDefMapsManifest>>,
    /// Exact cache-hit coverage plus a conservative seed for packages rebuilt immediately.
    pub(super) body_ir_coverage: Vec<PackageBodiesCoverage>,
}

impl PackageBuildPlan {
    /// Decides which packages still need source lowering for this build.
    ///
    /// For cache hits we also restore the parse snapshot from the artifact. That keeps source file
    /// ids, paths, and line indexes in sync with the offloaded phase payloads that lazy readers will
    /// load later.
    pub(super) fn build(
        startup_cache_load: StartupCacheLoad,
        body_ir_policy: BodyIrBuildPolicy,
        package_residency: &PackageResidencyPlan,
        cache_plan: &WorkspaceCachePlan,
        cache_store: &PackageCacheStore,
        workspace: &WorkspaceMetadata,
        parse: &mut ParseDb,
    ) -> Self {
        let package_count = parse.package_count();
        let package_selections = if startup_cache_load.is_enabled() {
            let mut cache_probe = StartupCacheProbe::new(
                package_count,
                body_ir_policy,
                package_residency,
                cache_plan,
                cache_store,
                workspace,
                parse,
            );
            cache_probe.select()
        } else {
            (0..package_count)
                .map(|_| StartupPackageSelection::BuildFromSource)
                .collect()
        };
        assert_eq!(
            package_selections.len(),
            package_count,
            "startup selection should cover every package slot",
        );
        let source_packages = PhasePackageSet::from_packages(
            package_selections
                .iter()
                .enumerate()
                .filter_map(|(package_idx, selection)| {
                    matches!(selection, StartupPackageSelection::BuildFromSource)
                        .then_some(PackageSlot(package_idx))
                })
                .collect(),
        );

        // Shape both provisional stores in package-slot order. Cache hits keep the DefMap manifest
        // and exact Body IR coverage accepted by probing. Source packages have no valid old
        // manifest and receive conservative Body coverage that their build output replaces.
        let (def_map_manifests, body_ir_coverage) = parse
            .packages()
            .iter()
            .zip(package_selections)
            .map(|(package, selection)| match selection {
                StartupPackageSelection::Cached {
                    body_ir_coverage,
                    def_map_manifest,
                } => (Some(def_map_manifest), body_ir_coverage),
                StartupPackageSelection::BuildFromSource => (
                    None,
                    PackageBodiesCoverage::from_crates(
                        package
                            .targets()
                            .iter()
                            .map(|target| {
                                if body_ir_policy.should_lower_target(package, target) {
                                    CrateBodiesCoverage::Missing
                                } else {
                                    CrateBodiesCoverage::SkippedByPolicy
                                }
                            })
                            .collect(),
                    ),
                ),
            })
            .unzip();

        Self {
            source_packages,
            def_map_manifests,
            body_ir_coverage,
        }
    }
}
