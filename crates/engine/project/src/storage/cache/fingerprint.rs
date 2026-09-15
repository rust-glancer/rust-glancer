//! Stable fingerprints for cache keys.
//!
//! Fingerprints are built from explicit field tags and length-prefixed values. This keeps cache
//! paths independent from Rust's `Hash`, debug formatting, and future serialization bytes.

use rg_std::{MemorySize, NativeOsString};
use std::{fmt, path::Path};
use wincode::{SchemaRead, SchemaWrite};

use crate::PackageResidencyPolicy;

use super::{
    CachedCfgOptions, CachedDependency, CachedPackage, CachedPath, CachedTarget,
    WorkspaceCachePlan, cached::CachedCfgKeyValue,
};

/// Stable BLAKE3 fingerprint used by cache keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
#[memsize(leaf)]
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

/// Adds tagged fields to one domain-separated stable fingerprint.
pub(crate) struct FingerprintBuilder {
    hasher: blake3::Hasher,
}

impl FingerprintBuilder {
    /// Builds the stable identity of one reusable package-cache generation.
    pub(super) fn cache_generation(
        cache_plan: &WorkspaceCachePlan,
        residency_policy: PackageResidencyPolicy,
    ) -> Fingerprint {
        let mut builder = Self::new("cache-generation");

        builder.bytes(
            "workspace.graph",
            Self::workspace_graph(cache_plan).as_bytes(),
        );
        // Different residency policies require different data to be cached. Including the policy
        // here intentionally invalidates the cache when the configuration changes.
        builder.str("package.residency", residency_policy.config_name());

        builder.finalize()
    }

    pub(super) fn workspace_graph(cache_plan: &WorkspaceCachePlan) -> Fingerprint {
        let mut builder = Self::new("workspace-graph");

        builder.usize("packages.len", cache_plan.packages.len());
        for package in &cache_plan.packages {
            builder.bytes(
                "package.identity",
                Self::package_identity(package).as_bytes(),
            );
        }

        builder.finalize()
    }

    pub(super) fn package_identity(package: &CachedPackage) -> Fingerprint {
        let mut builder = Self::new("package-identity");

        builder.u64("package.slot", package.package.0);
        builder.str("package.name", &package.name);
        builder.str("package.source", &package.source.to_string());
        builder.str("package.edition", &package.edition.to_string());
        builder.cached_path("package.manifest_path", &package.manifest_path);
        builder.cfg_options(&package.cfg_options);

        let targets = CachedTarget::sorted(&package.targets);
        builder.usize("targets.len", targets.len());
        for target in targets {
            builder.target(target);
        }

        let dependencies = CachedDependency::sorted(&package.dependencies);
        builder.usize("dependencies.len", dependencies.len());
        for dependency in dependencies {
            builder.dependency(dependency);
        }

        builder.finalize()
    }

    pub(super) fn package_source(
        workspace_root: &Path,
        cached_package: &CachedPackage,
        package: &rg_parse::Package,
    ) -> anyhow::Result<Fingerprint> {
        let mut builder = Self::new("package-source");
        let mut files = package.parsed_files().collect::<Vec<_>>();
        files.sort_by(|left, right| left.path().cmp(right.path()));

        // Package artifacts retain semantic analysis for an exact source snapshot. Cargo metadata
        // chooses the artifact path, while this fingerprint rejects stale bytes after source-only
        // edits that keep the package graph unchanged. Because it is computed again after DefMap's
        // late-source fixed point, generated module and included build-output files participate
        // like every other captured source file.
        builder.bytes(
            "package.identity",
            Self::package_identity(cached_package).as_bytes(),
        );
        builder.usize("files.len", files.len());
        for file in files {
            builder.cached_path(
                "file.path",
                &CachedPath::from_workspace_path(workspace_root, file.path()),
            );
            builder.bytes("file.source", file.source_revision().as_bytes());
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

        builder.bytes(
            "package.identity",
            Self::package_identity(package).as_bytes(),
        );
        builder.usize("files.len", files.len());

        // The artifact manifest is the authoritative file set for cache validation. Fresh parse
        // metadata initially knows only target roots, so using it here would miss edits in
        // out-of-line modules and incorrectly accept stale analysis payloads. Keep the same stable
        // path ordering as fresh source fingerprints so equivalent file sets hash identically.
        //
        // We do not care about weird scenarios such as adding `mod foo;`, saving the parent,
        // disabling the engine, creating `foo.rs` with the editor closed, reopening the engine,
        // and then being surprised that `foo.rs` is not discovered. Whoever does that can hit
        // Ctrl+S and enjoy the rebuilt cache. This is an absurd scenario that does not happen in
        // sane reality and is not worth supporting by persisting negative module paths.
        for file in files {
            builder.cached_path(
                "file.path",
                &CachedPath::from_workspace_path(workspace_root, file.path()),
            );
            builder.bytes(
                "file.source",
                file.source_descriptor().revision().as_bytes(),
            );
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

    fn target(&mut self, target: &CachedTarget) {
        self.str("target.name", &target.name);
        self.str("target.kind", &target.kind.to_string());
        self.cached_path("target.src_path", &target.src_path);
    }

    fn dependency(&mut self, dependency: &CachedDependency) {
        self.u64("dependency.package_slot", dependency.package.0);
        self.str("dependency.name", &dependency.name);
        self.bool("dependency.is_normal", dependency.is_normal);
        self.bool("dependency.is_build", dependency.is_build);
        self.bool("dependency.is_dev", dependency.is_dev);
    }

    fn cfg_options(&mut self, options: &CachedCfgOptions) {
        let mut atoms = options.atoms().iter().collect::<Vec<_>>();
        atoms.sort();

        self.usize("cfg_options.atoms.len", atoms.len());
        for atom in atoms {
            self.str("cfg_options.atom", atom);
        }

        let key_values = CachedCfgKeyValue::sorted(options.key_values());
        self.usize("cfg_options.key_values.len", key_values.len());
        for key_value in key_values {
            self.str("cfg_options.key_value.key", &key_value.key);
            self.str("cfg_options.key_value.value", &key_value.value);
        }
    }

    /// Hashes path kind, component boundaries, native encoding, and native units explicitly.
    fn cached_path(&mut self, field: &str, path: &CachedPath) {
        match path {
            CachedPath::WorkspaceRelative(components) => {
                self.bytes(field, &[0]);
                self.usize("path.components.len", components.len());
                for component in components {
                    self.native_os_string("path.component", component);
                }
            }
            CachedPath::NativeAbsolute(path) => {
                self.bytes(field, &[1]);
                self.native_os_string("path.absolute", path);
            }
        }
    }

    fn native_os_string(&mut self, field: &str, value: &NativeOsString) {
        self.bytes(field, value.as_encoded_bytes());
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
