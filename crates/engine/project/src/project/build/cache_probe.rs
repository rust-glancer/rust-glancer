//! Startup cache probing for fresh project builds.

use rg_body_ir::BodyIrBuildPolicy;
use rg_def_map::PackageSlot;
use rg_parse::{PackageParseSnapshot, ParseDb};
use rg_workspace::WorkspaceMetadata;

use crate::{
    PackageResidency, PackageResidencyPlan,
    cache::{CachedPackage, PackageCacheProbe, PackageCacheStore, WorkspaceCachePlan},
    profile::metric,
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
        let probe = Self {
            body_ir_policy,
            package_residency,
            cache_plan,
            cache_store,
            workspace,
            parse,
        };
        metric::CACHE_PROBE_PACKAGES.add(package_count as u64);
        probe
    }

    /// Chooses cache hits for the package graph before restoring any parse snapshot.
    ///
    /// A package-local cache hit is only tentative. If one dependency must rebuild, every reverse
    /// dependent must rebuild with it because cached scopes can retain dependency-local arena IDs.
    /// Parse restoration therefore happens only after the miss closure stops growing.
    pub(super) fn source_packages(&mut self) -> Vec<PackageSlot> {
        let package_count = self.parse.package_count();
        let mut probes = (0..package_count).map(|_| None).collect::<Vec<_>>();
        let mut source_packages = vec![false; package_count];

        for package_idx in 0..package_count {
            let package = PackageSlot(package_idx);
            let Some(probe) = self.probe_package(package) else {
                source_packages[package_idx] = true;
                continue;
            };
            probes[package_idx] = Some(probe);
        }

        loop {
            self.expand_reverse_dependents(&mut source_packages);

            // Restore into a private parse candidate. A late source mismatch can invalidate cache
            // hits visited earlier in this loop; discarding the candidate keeps those tentative
            // snapshots out of the source-build path.
            let mut candidate = self.parse.clone();
            let mut found_new_miss = false;
            for package_idx in 0..package_count {
                if source_packages[package_idx] {
                    continue;
                }
                let package = PackageSlot(package_idx);
                let Some(probe) = &probes[package_idx] else {
                    source_packages[package_idx] = true;
                    found_new_miss = true;
                    continue;
                };
                if !Self::restore_parse(&mut candidate, package, probe.parse.clone()) {
                    source_packages[package_idx] = true;
                    found_new_miss = true;
                }
            }

            if found_new_miss {
                continue;
            }

            *self.parse = candidate;
            metric::CACHE_PROBE_HITS.add(
                source_packages
                    .iter()
                    .filter(|must_build| !**must_build)
                    .count() as u64,
            );
            break;
        }

        source_packages
            .into_iter()
            .enumerate()
            .filter_map(|(package_idx, must_build)| must_build.then_some(PackageSlot(package_idx)))
            .collect()
    }

    /// Returns a package-local tentative hit without mutating the parse database.
    fn probe_package(&mut self, package: PackageSlot) -> Option<PackageCacheProbe> {
        if self.package_residency.package(package) != Some(PackageResidency::Offloadable) {
            metric::CACHE_PROBE_RESIDENT_PACKAGES.inc();
            return None;
        }
        metric::CACHE_PROBE_OFFLOADABLE_PACKAGES.inc();

        let Some(cached_package) = self.cache_plan.package(package) else {
            metric::CACHE_PROBE_UNPLANNED_PACKAGES.inc();
            return None;
        };
        let probe = self.read_probe(cached_package)?;
        if !self.snapshot_matches_header(&probe) {
            return None;
        }
        if !self.body_ir_matches_policy(package, &probe) {
            return None;
        }

        Some(probe)
    }

    fn read_probe(&mut self, package: &CachedPackage) -> Option<PackageCacheProbe> {
        // Cache reads fail open. A stale, corrupt, or missing artifact simply means this
        // offloadable package joins the source build and will overwrite its artifact later.
        let timer = metric::CACHE_PROBE_ARTIFACT_READ.start_timer();
        let probe = self.cache_store.read_probe_for_package(package);
        timer.finish();

        match probe {
            Ok(Some(probe)) => Some(probe),
            Ok(None) => {
                metric::CACHE_PROBE_MISSING_ARTIFACTS.inc();
                None
            }
            Err(_) => {
                metric::CACHE_PROBE_ARTIFACT_READ_ERRORS.inc();
                None
            }
        }
    }

    fn snapshot_matches_header(&mut self, probe: &PackageCacheProbe) -> bool {
        let timer = metric::CACHE_PROBE_SOURCE_FINGERPRINT.start_timer();
        let source_fingerprint = WorkspaceCachePlan::snapshot_source_fingerprint(
            self.workspace.workspace_root(),
            &probe.header.package,
            &probe.parse,
        );
        timer.finish();

        match source_fingerprint {
            Ok(fingerprint) if fingerprint == probe.header.source_fingerprint => true,
            Ok(_) => {
                metric::CACHE_PROBE_SOURCE_MISMATCHES.inc();
                false
            }
            Err(_) => {
                metric::CACHE_PROBE_SOURCE_ERRORS.inc();
                false
            }
        }
    }

    fn body_ir_matches_policy(&mut self, package: PackageSlot, probe: &PackageCacheProbe) -> bool {
        let parse_package = self
            .parse
            .package(package.0)
            .expect("startup cache probe package slot should exist in parse db");
        // A body artifact produced by a narrower target policy can still be structurally valid.
        // Validate every aligned Cargo target so configured secondary skips remain reusable while
        // an exhaustive all-target build rejects those same skips.
        let matches_policy = probe.body_ir_coverage.len() == parse_package.targets().len()
            && probe
                .body_ir_coverage
                .iter()
                .zip(parse_package.targets())
                .all(|(coverage, target)| {
                    coverage.is_complete()
                        || (!self
                            .body_ir_policy
                            .should_lower_target(parse_package, target)
                            && matches!(coverage, rg_body_ir::CrateBodiesCoverage::SkippedByPolicy))
                });

        if !matches_policy {
            metric::CACHE_PROBE_BODY_IR_POLICY_MISMATCHES.inc();
        }

        matches_policy
    }

    fn restore_parse(
        parse: &mut ParseDb,
        package: PackageSlot,
        snapshot: PackageParseSnapshot,
    ) -> bool {
        // Phase artifacts are only useful if their parse metadata can be mapped back to the current
        // ParseDb package slot. If that fails, the source build path recreates a coherent set.
        let timer = metric::CACHE_PROBE_PARSE_RESTORE.start_timer();
        let restored = parse.apply_package_parse_snapshot(package.0, snapshot);
        timer.finish();

        match restored {
            Ok(()) => true,
            Err(_) => {
                metric::CACHE_PROBE_PARSE_RESTORE_ERRORS.inc();
                false
            }
        }
    }

    fn expand_reverse_dependents(&self, source_packages: &mut [bool]) {
        let roots = source_packages
            .iter()
            .copied()
            .enumerate()
            .filter_map(|(package_idx, must_build)| must_build.then_some(package_idx))
            .filter_map(|package_idx| self.workspace.packages().get(package_idx))
            .map(|package| package.id.clone())
            .collect::<Vec<_>>();

        for package_idx in self.workspace.reverse_dependency_closure(&roots) {
            let Some(must_build) = source_packages.get_mut(package_idx) else {
                continue;
            };
            if !*must_build {
                *must_build = true;
                metric::CACHE_PROBE_PROPAGATED_MISSES.inc();
            }
        }
    }
}
