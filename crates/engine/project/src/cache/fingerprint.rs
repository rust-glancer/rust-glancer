//! Stable fingerprints for cache keys.
//!
//! Fingerprints are built from explicit field tags and length-prefixed values. This keeps cache
//! paths independent from Rust's `Hash`, debug formatting, and future serialization bytes.

use std::{fmt, path::Path};

use anyhow::Context as _;

use super::{CachedDependency, CachedPackage, CachedPackageId, CachedTarget, WorkspaceCachePlan};

/// Stable BLAKE3 fingerprint used by cache keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    #[cfg(test)]
    pub(super) fn from_stable_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }

        Ok(())
    }
}

pub(super) struct FingerprintBuilder {
    hasher: blake3::Hasher,
}

impl FingerprintBuilder {
    pub(super) fn workspace_graph(
        workspace_root: &Path,
        cache_plan: &WorkspaceCachePlan,
    ) -> Fingerprint {
        let mut builder = Self::new("workspace-graph");

        builder.usize("packages.len", cache_plan.packages.len());
        for package in &cache_plan.packages {
            builder.bytes(
                "package.identity",
                Self::package_identity(workspace_root, package).as_bytes(),
            );
        }

        builder.finalize()
    }

    pub(super) fn package_identity(workspace_root: &Path, package: &CachedPackage) -> Fingerprint {
        let mut builder = Self::new("package-identity");

        builder.u64("package.slot", package.package.0);
        builder.package_id("package.id", workspace_root, &package.package_id);
        builder.str("package.name", &package.name);
        builder.str("package.source", &package.source.to_string());
        builder.str("package.edition", &package.edition.to_string());
        builder.path(
            "package.manifest_path",
            workspace_root,
            package.manifest_path.as_path(),
        );

        let targets = CachedTarget::sorted(&package.targets);
        builder.usize("targets.len", targets.len());
        for target in targets {
            builder.target(workspace_root, target);
        }

        let dependencies = CachedDependency::sorted(&package.dependencies);
        builder.usize("dependencies.len", dependencies.len());
        for dependency in dependencies {
            builder.dependency(workspace_root, dependency);
        }

        builder.finalize()
    }

    pub(super) fn package_source(
        workspace_root: &Path,
        package: &rg_parse::Package,
    ) -> anyhow::Result<Fingerprint> {
        let mut builder = Self::new("package-source");
        let mut files = package.parsed_files().collect::<Vec<_>>();
        files.sort_by(|left, right| left.path().cmp(right.path()));

        // Package artifacts retain semantic analysis for a saved source snapshot. Cargo metadata
        // chooses the artifact path, while this fingerprint rejects stale bytes after source-only
        // edits that keep the package graph unchanged.
        builder.package_id(
            "package.id",
            workspace_root,
            &CachedPackageId::from_workspace(package.id()),
        );
        builder.usize("files.len", files.len());
        for file in files {
            builder.path("file.path", workspace_root, file.path());
            let source = std::fs::read(file.path()).with_context(|| {
                format!(
                    "while attempting to read {} for package cache source fingerprint",
                    file.path().display(),
                )
            })?;
            builder.bytes("file.source", &source);
        }

        Ok(builder.finalize())
    }

    pub(super) fn package_source_snapshot(
        workspace_root: &Path,
        package: &CachedPackage,
        snapshot: &rg_parse::PackageParseSnapshot,
    ) -> anyhow::Result<Fingerprint> {
        let mut builder = Self::new("package-source");
        let mut files = snapshot.files().iter().collect::<Vec<_>>();
        files.sort_by(|left, right| left.path().cmp(right.path()));

        builder.package_id("package.id", workspace_root, &package.package_id);
        builder.usize("files.len", files.len());

        // The artifact manifest is the authoritative file set for cache validation. Fresh parse
        // metadata initially knows only target roots, so using it here would miss edits in
        // out-of-line modules and incorrectly accept stale analysis payloads. Keep the same stable
        // path ordering as fresh source fingerprints so equivalent file sets hash identically.
        for file in files {
            builder.path("file.path", workspace_root, file.path());
            let source = std::fs::read(file.path()).with_context(|| {
                format!(
                    "while attempting to read {} for package cache source fingerprint",
                    file.path().display(),
                )
            })?;
            builder.bytes("file.source", &source);
        }

        Ok(builder.finalize())
    }

    fn new(domain: &str) -> Self {
        let mut this = Self {
            hasher: blake3::Hasher::new(),
        };
        this.str("domain", domain);
        this
    }

    fn target(&mut self, workspace_root: &Path, target: &CachedTarget) {
        self.str("target.name", &target.name);
        self.str("target.kind", &target.kind.to_string());
        self.path("target.src_path", workspace_root, target.src_path.as_path());
    }

    fn dependency(&mut self, workspace_root: &Path, dependency: &CachedDependency) {
        self.package_id(
            "dependency.package_id",
            workspace_root,
            &dependency.package_id,
        );
        self.str("dependency.name", &dependency.name);
        self.bool("dependency.is_normal", dependency.is_normal);
        self.bool("dependency.is_build", dependency.is_build);
        self.bool("dependency.is_dev", dependency.is_dev);
    }

    fn path(&mut self, field: &str, workspace_root: &Path, path: &Path) {
        let path = path.strip_prefix(workspace_root).unwrap_or(path);
        self.str(field, &path.display().to_string());
    }

    fn package_id(&mut self, field: &str, workspace_root: &Path, package_id: &CachedPackageId) {
        self.str(
            field,
            &Self::normalize_package_id(workspace_root, package_id),
        );
    }

    fn normalize_package_id(workspace_root: &Path, package_id: &CachedPackageId) -> String {
        let root_path = workspace_root.display().to_string();
        let mut root_paths = vec![root_path];

        // Cargo package IDs can preserve the non-canonical `/var` spelling on macOS while our
        // normalized workspace paths point at `/private/var`; both describe the same workspace.
        let public_tmp_path = root_paths[0]
            .strip_prefix("/private/")
            .map(|path| format!("/{path}"));
        if let Some(public_tmp_path) = public_tmp_path {
            root_paths.push(public_tmp_path);
        }

        let mut package_id = package_id.to_string();
        for root_path in &root_paths {
            package_id = package_id.replace(&format!("file://{root_path}"), "file://./");
        }
        for root_path in root_paths {
            package_id = package_id.replace(&root_path, ".");
        }

        package_id.replace("file://.//", "file://./")
    }

    fn str(&mut self, field: &str, value: &str) {
        self.bytes(field, value.as_bytes());
    }

    fn u64(&mut self, field: &str, value: u64) {
        self.bytes(field, &value.to_le_bytes());
    }

    fn usize(&mut self, field: &str, value: usize) {
        self.u64(
            field,
            u64::try_from(value).expect("cache identity counts should fit into u64"),
        );
    }

    fn bool(&mut self, field: &str, value: bool) {
        self.bytes(field, &[u8::from(value)]);
    }

    fn bytes(&mut self, field: &str, value: &[u8]) {
        self.hasher.update(field.as_bytes());
        self.hasher.update(&[0]);
        self.hasher.update(&(value.len() as u64).to_le_bytes());
        self.hasher.update(value);
    }

    fn finalize(self) -> Fingerprint {
        Fingerprint(*self.hasher.finalize().as_bytes())
    }
}
