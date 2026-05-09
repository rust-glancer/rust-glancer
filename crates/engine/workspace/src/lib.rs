use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt, io,
    path::{Path, PathBuf},
    process::Command,
};

mod memsize;
mod sysroot;

#[cfg(test)]
mod tests;

pub use self::sysroot::{SysrootCrate, SysrootSources};

/// Normalized workspace metadata used by the analysis pipeline.
///
/// This is our internal view of `cargo metadata`: it keeps only the fields and semantics the
/// later phases care about and avoids leaking Cargo's transport types throughout the codebase.
/// Filesystem roots are canonicalized at construction so save handling can compare paths directly
/// without each phase defending against Cargo's original path spelling. Missing non-workspace
/// targets are omitted because they cannot be parsed or reached through local save handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMetadata {
    workspace_root: PathBuf,
    packages: Vec<Package>,
    package_by_id: HashMap<PackageId, usize>,
}

/// Options used when asking Cargo for the workspace graph.
///
/// Cargo metadata includes dependencies for every platform unless callers pass
/// `--filter-platform`. Analysis wants one concrete graph, so the default resolves the current
/// rustc host triple and lets Cargo prune target-specific dependencies before lowering starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoMetadataConfig {
    target: CargoMetadataTarget,
}

impl CargoMetadataConfig {
    /// Uses an explicit target triple instead of auto-detecting the rustc host target.
    pub fn target_triple(mut self, target_triple: impl Into<String>) -> Self {
        let target_triple = target_triple.into().trim().to_string();
        self.target = if target_triple.is_empty() {
            CargoMetadataTarget::Auto
        } else {
            CargoMetadataTarget::Triple(target_triple)
        };
        self
    }

    /// Returns the configured target selection before auto-detection is resolved.
    pub fn target(&self) -> &CargoMetadataTarget {
        &self.target
    }

    /// Runs `cargo metadata` with the target-platform filter selected by this configuration.
    pub fn load_metadata(
        &self,
        manifest_path: impl AsRef<Path>,
    ) -> WorkspaceMetadataResult<cargo_metadata::Metadata> {
        let target_triple = self.resolved_target_triple()?;
        let mut command = cargo_metadata::MetadataCommand::new();
        command.manifest_path(manifest_path.as_ref().to_path_buf());
        command.other_options(vec!["--filter-platform".to_string(), target_triple]);
        command
            .exec()
            .map_err(WorkspaceMetadataError::CargoMetadata)
    }

    fn resolved_target_triple(&self) -> WorkspaceMetadataResult<String> {
        match &self.target {
            CargoMetadataTarget::Auto => detect_rustc_host_target(),
            CargoMetadataTarget::Triple(target_triple) => Ok(target_triple.clone()),
        }
    }
}

impl Default for CargoMetadataConfig {
    fn default() -> Self {
        Self {
            target: CargoMetadataTarget::Auto,
        }
    }
}

/// Target platform selection for Cargo metadata filtering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CargoMetadataTarget {
    /// Detect the host triple from `rustc -vV`.
    Auto,
    /// Pass this target triple to `cargo metadata --filter-platform`.
    Triple(String),
}

impl WorkspaceMetadata {
    /// Loads Cargo metadata from a manifest path and lowers it into the analysis metadata model.
    pub fn from_manifest_path(manifest_path: impl AsRef<Path>) -> WorkspaceMetadataResult<Self> {
        Self::from_manifest_path_with_config(manifest_path, &CargoMetadataConfig::default())
    }

    /// Loads Cargo metadata with explicit metadata options and lowers it into the internal model.
    pub fn from_manifest_path_with_config(
        manifest_path: impl AsRef<Path>,
        config: &CargoMetadataConfig,
    ) -> WorkspaceMetadataResult<Self> {
        let metadata = config.load_metadata(manifest_path)?;

        Self::from_cargo(metadata)
    }

    /// Lowers raw `cargo metadata` output into the project's normalized metadata model.
    pub fn from_cargo(metadata: cargo_metadata::Metadata) -> WorkspaceMetadataResult<Self> {
        let workspace_root = canonicalize_path(metadata.workspace_root.as_std_path())
            .map_err(WorkspaceMetadataError::Path)?;
        let workspace_members = metadata
            .workspace_members
            .iter()
            .map(PackageId::from_cargo)
            .collect::<HashSet<_>>();
        let dependencies_by_package = metadata
            .resolve
            .as_ref()
            .map(Self::lower_dependencies)
            .unwrap_or_default();

        let packages = metadata
            .packages
            .into_iter()
            .map(|package| {
                let package_id = PackageId::from_cargo(&package.id);
                let is_workspace_member = workspace_members.contains(&package_id);
                let raw_manifest_path = package.manifest_path.as_std_path();
                let manifest_path =
                    canonicalize_path(raw_manifest_path).map_err(WorkspaceMetadataError::Path)?;
                let raw_package_root = raw_manifest_path
                    .parent()
                    .expect("Cargo package manifest path should have a parent directory");
                let package_root = manifest_path
                    .parent()
                    .expect("canonical package manifest path should have a parent directory");
                let source = PackageSource::from_cargo_source(
                    &package_id,
                    is_workspace_member,
                    package.source.as_ref(),
                )?;
                let targets = package
                    .targets
                    .iter()
                    .map(|target| {
                        Target::from_cargo(
                            target,
                            raw_package_root,
                            package_root,
                            is_workspace_member,
                        )
                    })
                    .collect::<WorkspaceMetadataResult<Vec<_>>>()?
                    .into_iter()
                    .flatten()
                    .collect();

                Ok(Package {
                    id: package_id.clone(),
                    name: package.name.to_string(),
                    edition: RustEdition::from_cargo(package.edition),
                    origin: if is_workspace_member {
                        PackageOrigin::Workspace
                    } else {
                        PackageOrigin::Dependency
                    },
                    source,
                    is_workspace_member,
                    manifest_path,
                    targets,
                    dependencies: dependencies_by_package
                        .get(&package_id)
                        .cloned()
                        .unwrap_or_default(),
                })
            })
            .collect::<WorkspaceMetadataResult<Vec<_>>>()?;

        let package_by_id = packages
            .iter()
            .enumerate()
            .map(|(idx, package)| (package.id.clone(), idx))
            .collect();

        Ok(Self {
            workspace_root,
            packages,
            package_by_id,
        })
    }

    /// Returns this workspace with sysroot crates modeled as ordinary packages.
    pub fn with_sysroot_sources(mut self, sources: Option<SysrootSources>) -> Self {
        if let Some(sources) = sources {
            self.add_sysroot_sources(sources);
        }
        self
    }

    /// Returns sysroot source roots that were previously injected into this metadata graph.
    pub fn sysroot_sources(&self) -> Option<SysrootSources> {
        self.packages
            .iter()
            .find(|package| package.origin.is_sysroot())
            .and_then(|package| package.manifest_path.parent())
            .and_then(|crate_root| crate_root.parent())
            .and_then(SysrootSources::from_library_root)
    }

    /// Adds `core`, `alloc`, and `std` from rust-src and injects them into normal packages.
    pub fn add_sysroot_sources(&mut self, sources: SysrootSources) {
        if self
            .packages
            .iter()
            .any(|package| package.origin.is_sysroot())
        {
            return;
        }

        let mut sysroot_packages = SysrootCrate::ALL
            .iter()
            .copied()
            .map(|krate| Self::sysroot_package(&sources, krate))
            .collect::<Vec<_>>();

        for package in &mut self.packages {
            if package.origin.is_sysroot() {
                continue;
            }

            for krate in SysrootCrate::ALL {
                if package
                    .dependencies
                    .iter()
                    .any(|dependency| dependency.name() == krate.name())
                {
                    continue;
                }
                package
                    .dependencies
                    .push(PackageDependency::for_all_targets(
                        PackageId::sysroot(krate),
                        krate.name(),
                    ));
            }
        }

        self.packages.append(&mut sysroot_packages);
        self.rebuild_package_index();
    }

    fn sysroot_package(sources: &SysrootSources, krate: SysrootCrate) -> Package {
        let dependencies = match krate {
            SysrootCrate::Core => Vec::new(),
            SysrootCrate::Alloc => vec![PackageDependency::normal(
                PackageId::sysroot(SysrootCrate::Core),
                "core",
            )],
            SysrootCrate::Std => vec![
                PackageDependency::normal(PackageId::sysroot(SysrootCrate::Core), "core"),
                PackageDependency::normal(PackageId::sysroot(SysrootCrate::Alloc), "alloc"),
            ],
        };

        Package {
            id: PackageId::sysroot(krate),
            name: krate.name().to_string(),
            edition: RustEdition::Edition2024,
            origin: PackageOrigin::Sysroot(krate),
            source: PackageSource::Sysroot,
            is_workspace_member: false,
            manifest_path: sources.library_root().join(krate.name()).join("Cargo.toml"),
            targets: vec![Target {
                name: krate.name().to_string(),
                kind: TargetKind::Lib,
                src_path: sources.crate_root(krate),
            }],
            dependencies,
        }
    }

    fn rebuild_package_index(&mut self) {
        self.package_by_id = self
            .packages
            .iter()
            .enumerate()
            .map(|(idx, package)| (package.id.clone(), idx))
            .collect();
    }

    fn lower_dependencies(
        resolve: &cargo_metadata::Resolve,
    ) -> HashMap<PackageId, Vec<PackageDependency>> {
        resolve
            .nodes
            .iter()
            .map(|node| {
                (
                    PackageId::from_cargo(&node.id),
                    node.deps
                        .iter()
                        .map(PackageDependency::from_cargo)
                        .collect::<Vec<_>>(),
                )
            })
            .collect()
    }

    /// Returns the workspace root directory.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Returns all known packages.
    pub fn packages(&self) -> &[Package] {
        &self.packages
    }

    /// Returns one package by normalized package id.
    pub fn package(&self, package_id: &PackageId) -> Option<&Package> {
        let slot = self.package_by_id.get(package_id).copied()?;
        self.packages.get(slot)
    }

    /// Returns package slots whose manifest directory contains `path`.
    ///
    /// This is intentionally a filesystem-root query, not a parsed-file ownership query. The
    /// analysis host uses it when a saved file was not part of the parsed graph yet, for example
    /// after `mod api;` was saved before `api.rs` existed. Rebuilding the containing package lets
    /// normal module discovery decide whether the new path is actually reachable.
    pub fn package_slots_containing_path(&self, path: &Path) -> Vec<usize> {
        self.packages
            .iter()
            .enumerate()
            .filter_map(|(slot, package)| package.contains_path(path).then_some(slot))
            .collect()
    }

    /// Iterates over packages that belong to the analyzed workspace.
    pub fn workspace_packages(&self) -> impl Iterator<Item = &Package> + '_ {
        self.packages
            .iter()
            .filter(|package| package.is_workspace_member)
    }

    /// Returns package slots that should be refreshed after one or more packages change.
    ///
    /// Source changes can alter the public surface of the changed package, so every reverse
    /// dependent must be rebuilt against the new graph. The closure is intentionally package-wide:
    /// it is coarse enough to stay predictable while avoiding whole-workspace rebuilds on normal
    /// source edits.
    pub fn reverse_dependency_closure(&self, roots: &[PackageId]) -> Vec<usize> {
        let mut affected_ids = roots.iter().cloned().collect::<HashSet<_>>();

        loop {
            let previous_len = affected_ids.len();

            for package in &self.packages {
                if affected_ids.contains(&package.id) {
                    continue;
                }

                if package
                    .dependencies
                    .iter()
                    .any(|dependency| affected_ids.contains(dependency.package_id()))
                {
                    affected_ids.insert(package.id.clone());
                }
            }

            if affected_ids.len() == previous_len {
                break;
            }
        }

        self.packages
            .iter()
            .enumerate()
            .filter_map(|(package_slot, package)| {
                affected_ids.contains(&package.id).then_some(package_slot)
            })
            .collect()
    }
}

pub type WorkspaceMetadataResult<T> = Result<T, WorkspaceMetadataError>;

#[derive(Debug)]
pub enum WorkspaceMetadataError {
    CargoMetadata(cargo_metadata::Error),
    Path(io::Error),
    RustcHostTarget { error: io::Error },
    RustcHostTargetCommandFailed { status: String, stderr: String },
    RustcHostTargetMissing { stdout: String },
    UnsupportedPackageSource { package: PackageId, source: String },
}

impl fmt::Display for WorkspaceMetadataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CargoMetadata(error) => {
                write!(f, "while attempting to load Cargo metadata: {error}")
            }
            Self::Path(error) => write!(f, "{error}"),
            Self::RustcHostTarget { error } => {
                write!(f, "while attempting to detect rustc host target: {error}")
            }
            Self::RustcHostTargetCommandFailed { status, stderr } => {
                write!(
                    f,
                    "rustc -vV failed while detecting host target: status {status}, stderr: {stderr}"
                )
            }
            Self::RustcHostTargetMissing { stdout } => write!(
                f,
                "rustc -vV output did not contain a host target line: {stdout:?}"
            ),
            Self::UnsupportedPackageSource { package, source } => write!(
                f,
                "unsupported Cargo source `{source}` for package {package}"
            ),
        }
    }
}

impl Error for WorkspaceMetadataError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CargoMetadata(error) => Some(error),
            Self::Path(error) => Some(error),
            Self::RustcHostTarget { error } => Some(error),
            Self::RustcHostTargetCommandFailed { .. } | Self::RustcHostTargetMissing { .. } => None,
            Self::UnsupportedPackageSource { .. } => None,
        }
    }
}

fn detect_rustc_host_target() -> WorkspaceMetadataResult<String> {
    let output = Command::new("rustc")
        .arg("-vV")
        .output()
        .map_err(|error| WorkspaceMetadataError::RustcHostTarget { error })?;

    if !output.status.success() {
        return Err(WorkspaceMetadataError::RustcHostTargetCommandFailed {
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_rustc_host_target(&stdout).ok_or_else(|| WorkspaceMetadataError::RustcHostTargetMissing {
        stdout: stdout.into_owned(),
    })
}

fn parse_rustc_host_target(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.strip_prefix("host:").map(str::trim))
        .filter(|host| !host.is_empty())
        .map(str::to_string)
}

fn canonicalize_path(path: &Path) -> io::Result<PathBuf> {
    path.canonicalize().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "while attempting to canonicalize {}: {error}",
                path.display()
            ),
        )
    })
}

/// Stable package identifier derived from Cargo metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash, derive_more::Display)]
#[display("{_0}")]
pub struct PackageId(String);

impl PackageId {
    fn from_cargo(id: &cargo_metadata::PackageId) -> Self {
        Self(id.to_string())
    }

    fn sysroot(krate: SysrootCrate) -> Self {
        Self(format!("sysroot:{}", krate.name()))
    }
}

/// Stable slot of one package inside a normalized workspace metadata snapshot.
///
/// Slots are dense and snapshot-local. Rebuild code must rebuild the whole project when Cargo
/// metadata changes package ordering or membership, so analysis IDs never cross metadata graphs.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub struct PackageSlot(pub usize);

/// Where one normalized package came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageOrigin {
    Workspace,
    Dependency,
    Sysroot(SysrootCrate),
}

impl PackageOrigin {
    pub fn is_sysroot(&self) -> bool {
        matches!(self, Self::Sysroot(_))
    }
}

/// Cargo source kind used to classify packages for future residency/cache policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, derive_more::Display)]
pub enum PackageSource {
    #[display("workspace")]
    Workspace,
    #[display("path")]
    Path,
    #[display("registry")]
    Registry,
    #[display("sparse-registry")]
    SparseRegistry,
    #[display("git")]
    Git,
    #[display("local-registry")]
    LocalRegistry,
    #[display("directory")]
    Directory,
    #[display("sysroot")]
    Sysroot,
}

impl PackageSource {
    fn from_cargo_source(
        package: &PackageId,
        is_workspace_member: bool,
        source: Option<&cargo_metadata::Source>,
    ) -> WorkspaceMetadataResult<Self> {
        if is_workspace_member {
            return Ok(Self::Workspace);
        }

        let Some(source) = source else {
            return Ok(Self::Path);
        };
        let source = source.repr.as_str();

        if source.starts_with("path+") {
            Ok(Self::Path)
        } else if source.starts_with("registry+") {
            Ok(Self::Registry)
        } else if source.starts_with("sparse+") {
            Ok(Self::SparseRegistry)
        } else if source.starts_with("git+") {
            Ok(Self::Git)
        } else if source.starts_with("local-registry+") {
            Ok(Self::LocalRegistry)
        } else if source.starts_with("directory+") {
            Ok(Self::Directory)
        } else {
            Err(WorkspaceMetadataError::UnsupportedPackageSource {
                package: package.clone(),
                source: source.to_string(),
            })
        }
    }
}

/// Rust edition used by a package.
///
/// We keep this normalized instead of leaking `cargo_metadata::Edition` so later phases can ask
/// edition-shaped questions without depending on Cargo's transport model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, derive_more::Display)]
pub enum RustEdition {
    #[display("2015")]
    Edition2015,
    #[display("2018")]
    Edition2018,
    #[display("2021")]
    Edition2021,
    #[display("2024")]
    Edition2024,
}

impl RustEdition {
    fn from_cargo(edition: cargo_metadata::Edition) -> Self {
        match edition {
            cargo_metadata::Edition::E2015 => Self::Edition2015,
            cargo_metadata::Edition::E2018 => Self::Edition2018,
            cargo_metadata::Edition::E2021 => Self::Edition2021,
            cargo_metadata::Edition::E2024 => Self::Edition2024,
            // Cargo parses a few future-edition placeholders. Until rust-src exposes matching
            // prelude modules, resolve them through the newest edition we understand.
            _ => Self::Edition2024,
        }
    }

    pub fn prelude_module(self) -> &'static str {
        match self {
            Self::Edition2015 => "rust_2015",
            Self::Edition2018 => "rust_2018",
            Self::Edition2021 => "rust_2021",
            Self::Edition2024 => "rust_2024",
        }
    }
}

/// Normalized package metadata relevant to later analysis phases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub id: PackageId,
    pub name: String,
    pub edition: RustEdition,
    pub origin: PackageOrigin,
    pub source: PackageSource,
    pub is_workspace_member: bool,
    pub manifest_path: PathBuf,
    pub targets: Vec<Target>,
    pub dependencies: Vec<PackageDependency>,
}

impl Package {
    /// Returns the package root directory, modeled as the parent of `Cargo.toml`.
    pub fn root_dir(&self) -> &Path {
        self.manifest_path
            .parent()
            .expect("package manifest path should have a parent directory")
    }

    fn contains_path(&self, path: &Path) -> bool {
        path.starts_with(self.root_dir())
    }
}

/// Normalized target metadata with one target kind per target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    pub kind: TargetKind,
    pub src_path: PathBuf,
}

impl Target {
    fn from_cargo(
        target: &cargo_metadata::Target,
        raw_package_root: &Path,
        package_root: &Path,
        is_workspace_member: bool,
    ) -> WorkspaceMetadataResult<Option<Self>> {
        let Some(src_path) = normalize_target_src_path(
            target.src_path.as_std_path(),
            raw_package_root,
            package_root,
            is_workspace_member,
        )?
        else {
            return Ok(None);
        };

        Ok(Some(Self {
            name: target.name.to_string(),
            kind: TargetKind::from_cargo(target),
            src_path,
        }))
    }
}

fn normalize_target_src_path(
    path: &Path,
    raw_package_root: &Path,
    package_root: &Path,
    is_workspace_member: bool,
) -> WorkspaceMetadataResult<Option<PathBuf>> {
    match canonicalize_path(path) {
        Ok(path) => Ok(Some(path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound && !is_workspace_member => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // Keep workspace target identity stable across the edit that declares a target and the
            // later edit that materializes its file. Non-workspace targets do not participate in
            // that save flow, so missing ones are filtered out above.
            let relative_path = path
                .strip_prefix(raw_package_root)
                .map_err(|_| WorkspaceMetadataError::Path(error))?;
            Ok(Some(package_root.join(relative_path)))
        }
        Err(error) => Err(WorkspaceMetadataError::Path(error)),
    }
}

/// One dependency edge after Cargo resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageDependency {
    package: PackageId,
    name: String,
    is_normal: bool,
    is_build: bool,
    is_dev: bool,
}

impl PackageDependency {
    fn from_cargo(dependency: &cargo_metadata::NodeDep) -> Self {
        let mut is_normal = dependency.dep_kinds.is_empty();
        let mut is_build = false;
        let mut is_dev = false;

        // Cargo may report separate platform-specific entries for the same dependency kind.
        // Until we analyze a concrete target platform, each listed kind is potentially relevant.
        for kind in &dependency.dep_kinds {
            match kind.kind {
                cargo_metadata::DependencyKind::Normal => is_normal = true,
                cargo_metadata::DependencyKind::Development => is_dev = true,
                cargo_metadata::DependencyKind::Build => is_build = true,
                // Keep future Cargo dependency kinds resolvable instead of silently dropping them.
                cargo_metadata::DependencyKind::Unknown => is_normal = true,
            }
        }

        Self {
            package: PackageId::from_cargo(&dependency.pkg),
            name: dependency.name.clone(),
            is_normal,
            is_build,
            is_dev,
        }
    }

    fn normal(package: PackageId, name: impl Into<String>) -> Self {
        Self {
            package,
            name: name.into(),
            is_normal: true,
            is_build: false,
            is_dev: false,
        }
    }

    fn for_all_targets(package: PackageId, name: impl Into<String>) -> Self {
        Self {
            package,
            name: name.into(),
            is_normal: true,
            is_build: true,
            is_dev: true,
        }
    }

    /// Returns the resolved package this dependency points to.
    pub fn package_id(&self) -> &PackageId {
        &self.package
    }

    /// Returns the crate name used by source code to refer to this dependency.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns whether this edge is visible to normal package targets.
    pub fn is_normal(&self) -> bool {
        self.is_normal
    }

    /// Returns whether this edge is visible to build scripts.
    pub fn is_build(&self) -> bool {
        self.is_build
    }

    /// Returns whether this edge is visible to dev targets.
    pub fn is_dev(&self) -> bool {
        self.is_dev
    }

    /// Returns whether this dependency can be named from a target of the given kind.
    pub fn applies_to_target(&self, target_kind: &TargetKind) -> bool {
        match target_kind {
            TargetKind::CustomBuild => self.is_build,
            TargetKind::Example | TargetKind::Test | TargetKind::Bench => {
                self.is_normal || self.is_dev
            }
            TargetKind::Lib | TargetKind::Bin | TargetKind::Other(_) => self.is_normal,
        }
    }
}

/// Analysis-relevant target kinds.
///
/// We intentionally support less kinds than `cargo_metadata`,
/// since we are only interested in the kinds that are useful
/// for analysis.
#[derive(Debug, Clone, PartialEq, Eq, Hash, derive_more::Display)]
pub enum TargetKind {
    #[display("lib")]
    Lib,
    #[display("bin")]
    Bin,
    #[display("example")]
    Example,
    #[display("test")]
    Test,
    #[display("bench")]
    Bench,
    #[display("custom-build")]
    CustomBuild,
    #[display("{_0}")]
    Other(String),
}

impl TargetKind {
    fn from_cargo(target: &cargo_metadata::Target) -> Self {
        if target.is_kind(cargo_metadata::TargetKind::Lib) {
            Self::Lib
        } else if target.is_kind(cargo_metadata::TargetKind::Bin) {
            Self::Bin
        } else if target.is_kind(cargo_metadata::TargetKind::Example) {
            Self::Example
        } else if target.is_kind(cargo_metadata::TargetKind::Test) {
            Self::Test
        } else if target.is_kind(cargo_metadata::TargetKind::Bench) {
            Self::Bench
        } else if target.is_kind(cargo_metadata::TargetKind::CustomBuild) {
            Self::CustomBuild
        } else {
            let fallback = target
                .kind
                .first()
                .map(|kind| kind.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            Self::Other(fallback)
        }
    }

    pub fn is_lib(&self) -> bool {
        matches!(self, Self::Lib)
    }

    pub fn is_custom_build(&self) -> bool {
        matches!(self, Self::CustomBuild)
    }

    // Used for predictable ordering, e.g.
    // in test snapshots.
    pub fn sort_order(&self) -> u8 {
        match self {
            Self::Lib => 0,
            Self::Bin => 1,
            Self::Example => 2,
            Self::Test => 3,
            Self::Bench => 4,
            Self::CustomBuild => 5,
            Self::Other(_) => 6,
        }
    }
}
