//! Workspace cache planning from normalized workspace metadata.
//!
//! This module is the conversion boundary into the cache schema. Cargo/workspace metadata supplies
//! package identity and dependency edges. The parse crate supplies the target-selection rule so
//! cache planning can predict the artifact graph before a `ParseDb` exists.

use std::path::Path;

use rg_parse::{PackageParseSnapshot, ParseDb};
use rg_workspace::{PackageSlot, WorkspaceMetadata};

use super::{
    CachedDependency, CachedPackage, CachedPackageId, CachedPackageSlot, CachedPackageSource,
    CachedPath, CachedRustEdition, CachedTarget, CachedTargetKind, Fingerprint, PackageCacheHeader,
    fingerprint,
};

/// Cache-schema plan for the package artifacts belonging to one workspace graph.
///
/// This is the deterministic, in-memory view used to name package artifacts and reject artifacts
/// whose package/target graph no longer matches the current Cargo metadata snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCachePlan {
    pub(crate) packages: Vec<CachedPackage>,
}

impl WorkspaceCachePlan {
    /// Builds cache metadata for the package targets analyzed by the current project.
    ///
    /// Cargo metadata can list dependency examples, tests, benches, and binaries that we do not
    /// parse for non-workspace packages. The target list follows `rg_parse::Package` target
    /// selection, which keeps package-artifact identities aligned with fresh builds and future
    /// artifact-backed restores.
    pub fn build(workspace: &WorkspaceMetadata) -> Self {
        let packages = workspace
            .packages()
            .iter()
            .enumerate()
            .map(|(package_slot, package)| CachedPackage {
                package: CachedPackageSlot::from_workspace(PackageSlot(package_slot)),
                package_id: CachedPackageId::from_workspace(&package.id),
                name: package.name.clone(),
                source: CachedPackageSource::from(package.source),
                edition: CachedRustEdition::from(package.edition),
                manifest_path: CachedPath::from_workspace_path(&package.manifest_path),
                targets: rg_parse::Package::analyzed_targets(package)
                    .iter()
                    .map(CachedTarget::from_workspace_target)
                    .collect(),
                dependencies: package
                    .dependencies
                    .iter()
                    .map(|dependency| CachedDependency {
                        package_id: CachedPackageId::from_workspace(dependency.package_id()),
                        name: dependency.name().to_string(),
                        is_normal: dependency.is_normal(),
                        is_build: dependency.is_build(),
                        is_dev: dependency.is_dev(),
                    })
                    .collect(),
            })
            .collect();

        Self { packages }
    }

    /// Returns all cached packages in `WorkspaceMetadata::packages()` order.
    #[cfg(test)]
    pub(super) fn packages(&self) -> &[CachedPackage] {
        &self.packages
    }

    /// Returns one cached package by stable package slot.
    pub fn package(&self, package: PackageSlot) -> Option<&CachedPackage> {
        self.packages.get(package.0)
    }

    /// Computes source fingerprints for all parsed packages in slot order.
    ///
    /// Package identities deliberately ignore source text so graph-equivalent saves reuse artifact
    /// paths. These fingerprints are the second half of validation: a cache hit must match both
    /// the Cargo graph and the current source snapshot.
    pub fn source_fingerprints(
        &self,
        workspace_root: &Path,
        parse: &ParseDb,
    ) -> anyhow::Result<Vec<Option<Fingerprint>>> {
        parse
            .packages()
            .iter()
            .enumerate()
            .map(|(package_idx, package)| {
                if self.package(PackageSlot(package_idx)).is_none() {
                    return Ok(None);
                }

                fingerprint::FingerprintBuilder::package_source(workspace_root, package).map(Some)
            })
            .collect()
    }

    /// Refreshes source fingerprints for selected parsed packages.
    pub fn refresh_source_fingerprints(
        &self,
        workspace_root: &Path,
        parse: &ParseDb,
        fingerprints: &mut [Option<Fingerprint>],
        packages: &[PackageSlot],
    ) -> anyhow::Result<()> {
        for package in packages {
            let Some(slot) = fingerprints.get_mut(package.0) else {
                continue;
            };
            let Some(parse_package) = parse.package(package.0) else {
                *slot = None;
                continue;
            };
            if self.package(*package).is_none() {
                *slot = None;
                continue;
            }

            *slot = Some(fingerprint::FingerprintBuilder::package_source(
                workspace_root,
                parse_package,
            )?);
        }

        Ok(())
    }

    pub(crate) fn snapshot_source_fingerprint(
        workspace_root: &Path,
        package: &CachedPackage,
        snapshot: &PackageParseSnapshot,
    ) -> anyhow::Result<Fingerprint> {
        fingerprint::FingerprintBuilder::package_source_snapshot(workspace_root, package, snapshot)
    }

    /// Builds an artifact header for one package bundle and source snapshot.
    pub fn artifact_header(
        &self,
        package: PackageSlot,
        source_fingerprints: &[Option<Fingerprint>],
    ) -> Option<PackageCacheHeader> {
        let source_fingerprint = source_fingerprints.get(package.0).copied().flatten()?;
        Some(PackageCacheHeader::new(
            self.package(package)?.clone(),
            source_fingerprint,
        ))
    }

    /// Returns the cache generation fingerprint for this workspace graph.
    ///
    /// Source edits keep this stable, while package/target/dependency metadata changes select a
    /// new artifact directory and make old generations eligible for cleanup.
    pub fn fingerprint(&self, workspace_root: &Path) -> Fingerprint {
        fingerprint::FingerprintBuilder::workspace_graph(workspace_root, self)
    }
}

impl CachedTarget {
    fn from_workspace_target(target: &rg_workspace::Target) -> Self {
        Self {
            name: target.name.clone(),
            kind: CachedTargetKind::from_workspace(&target.kind),
            src_path: CachedPath::from_workspace_path(&target.src_path),
        }
    }
}
