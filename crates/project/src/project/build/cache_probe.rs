//! Startup cache probing for fresh project builds.

use rg_body_ir::{BodyIrBuildPolicy, TargetBodiesStatus};
use rg_def_map::PackageSlot;
use rg_parse::{PackageParseSnapshot, ParseDb};
use rg_workspace::WorkspaceMetadata;

use crate::{
    PackageResidency, PackageResidencyPlan,
    cache::{
        CachedPackage, PackageCacheArtifact, PackageCacheBodyIrState, PackageCacheStore,
        WorkspaceCachePlan,
    },
    profile::{CacheProbeProfile, CacheProbeRecorder},
};

/// Checks whether offloadable packages can be seeded from existing cache artifacts.
///
/// A probe hit restores parse metadata and lets later phase stores lazy-load the heavier payloads
/// from disk. Any cache uncertainty is treated as a miss so the package rebuilds from source.
pub(super) struct StartupCacheProbe<'a> {
    body_ir_policy: BodyIrBuildPolicy,
    package_residency: &'a PackageResidencyPlan,
    cache_plan: &'a WorkspaceCachePlan,
    cache_store: &'a PackageCacheStore,
    workspace: &'a WorkspaceMetadata,
    parse: &'a mut ParseDb,
    profile: CacheProbeRecorder,
}

impl<'a> StartupCacheProbe<'a> {
    pub(super) fn new(
        package_count: usize,
        body_ir_policy: BodyIrBuildPolicy,
        package_residency: &'a PackageResidencyPlan,
        cache_plan: &'a WorkspaceCachePlan,
        cache_store: &'a PackageCacheStore,
        workspace: &'a WorkspaceMetadata,
        parse: &'a mut ParseDb,
    ) -> Self {
        Self {
            body_ir_policy,
            package_residency,
            cache_plan,
            cache_store,
            workspace,
            parse,
            profile: CacheProbeRecorder::new(package_count),
        }
    }

    /// Returns whether this package must go through the normal source build path.
    pub(super) fn should_build_from_source(&mut self, package: PackageSlot) -> bool {
        if self.package_residency.package(package) != Some(PackageResidency::Offloadable) {
            self.profile.resident_package();
            return true;
        }
        self.profile.offloadable_package();

        let Some(cached_package) = self.cache_plan.package(package) else {
            self.profile.unplanned_package();
            return true;
        };

        let Some(artifact) = self.read_artifact(cached_package) else {
            return true;
        };
        if !self.source_matches(&artifact) {
            return true;
        }
        if !self.body_ir_matches_policy(package, &artifact) {
            return true;
        }
        if !self.restore_parse(package, artifact.payload.parse) {
            return true;
        }

        self.profile.hit();
        false
    }

    pub(super) fn finish(self) -> Option<CacheProbeProfile> {
        self.profile.finish()
    }

    fn read_artifact(&mut self, package: &CachedPackage) -> Option<PackageCacheArtifact> {
        // Cache reads fail open. A stale, corrupt, or missing artifact simply means this
        // offloadable package joins the source build and will overwrite its artifact later.
        match self
            .profile
            .time_artifact_read(|| self.cache_store.read_artifact_for_package(package))
        {
            Ok(Some(artifact)) => Some(artifact),
            Ok(None) => {
                self.profile.missing_artifact();
                None
            }
            Err(_) => {
                self.profile.artifact_read_error();
                None
            }
        }
    }

    fn source_matches(&mut self, artifact: &PackageCacheArtifact) -> bool {
        let source_fingerprint = self.profile.time_source_fingerprint(|| {
            WorkspaceCachePlan::snapshot_source_fingerprint(
                self.workspace.workspace_root(),
                &artifact.header.package,
                &artifact.payload.parse,
            )
        });

        match source_fingerprint {
            Ok(fingerprint) if fingerprint == artifact.header.source_fingerprint => true,
            Ok(_) => {
                self.profile.source_mismatch();
                false
            }
            Err(_) => {
                self.profile.source_error();
                false
            }
        }
    }

    fn body_ir_matches_policy(
        &mut self,
        package: PackageSlot,
        artifact: &PackageCacheArtifact,
    ) -> bool {
        let parse_package = self
            .parse
            .package(package.0)
            .expect("startup cache probe package slot should exist in parse db");
        if !self.body_ir_policy.should_lower_package(parse_package) {
            return true;
        }

        // A body artifact produced by a narrower policy can still be structurally valid while
        // containing skipped targets. Reject it so the requested policy gets a full source rebuild.
        let matches_policy = match &artifact.payload.body_ir {
            PackageCacheBodyIrState::Built(bundle) => bundle
                .package()
                .targets()
                .iter()
                .all(|target| target.status() == TargetBodiesStatus::Built),
            PackageCacheBodyIrState::SkippedByPolicy => false,
        };

        if !matches_policy {
            self.profile.body_ir_policy_mismatch();
        }

        matches_policy
    }

    fn restore_parse(&mut self, package: PackageSlot, snapshot: PackageParseSnapshot) -> bool {
        // Phase artifacts are only useful if their parse metadata can be mapped back to the current
        // ParseDb package slot. If that fails, the source build path recreates a coherent set.
        match self
            .profile
            .time_parse_restore(|| self.parse.apply_package_parse_snapshot(package.0, snapshot))
        {
            Ok(()) => true,
            Err(_) => {
                self.profile.restore_error();
                false
            }
        }
    }
}
