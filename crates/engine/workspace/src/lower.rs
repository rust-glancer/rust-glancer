use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
};

use rg_cfg_eval::CfgOptions;
use rg_std::MemorySize;

use crate::{
    Package, PackageDependency, PackageId, PackageOrigin, PackageSource, RustEdition, Target,
    TargetKind, WorkspaceMetadata, WorkspaceMetadataError, WorkspaceMetadataResult,
    path::canonicalize_path,
};

/// Analysis-facing cfg options applied while lowering Cargo metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, MemorySize)]
pub struct WorkspaceLoweringConfig {
    cfg_test: bool,
}

impl WorkspaceLoweringConfig {
    pub fn cfg_test(mut self, enabled: bool) -> Self {
        self.cfg_test = enabled;
        self
    }

    pub fn is_cfg_test_enabled(self) -> bool {
        self.cfg_test
    }
}

impl WorkspaceMetadata {
    /// Lowers raw Cargo metadata into the normalized workspace model.
    pub fn lower(
        metadata: cargo_metadata::Metadata,
        target_cfg: CfgOptions,
        config: WorkspaceLoweringConfig,
    ) -> WorkspaceMetadataResult<Self> {
        CargoMetadataLowerer::lower(metadata, target_cfg, config)
    }

    /// Lowers fixture metadata with current-host cfg facts.
    ///
    /// This is intended for tests, where fixture metadata is generated for the current host. Real
    /// Cargo metadata loading should pass the cfg facts returned by `CargoMetadataConfig`.
    pub fn for_tests(
        metadata: cargo_metadata::Metadata,
        config: WorkspaceLoweringConfig,
    ) -> WorkspaceMetadataResult<Self> {
        Self::lower(metadata, CfgOptions::current_host(), config)
    }
}

impl From<cargo_metadata::PackageId> for PackageId {
    fn from(id: cargo_metadata::PackageId) -> Self {
        Self(id.repr)
    }
}

impl From<&cargo_metadata::PackageId> for PackageId {
    fn from(id: &cargo_metadata::PackageId) -> Self {
        id.clone().into()
    }
}

impl From<cargo_metadata::Edition> for RustEdition {
    fn from(edition: cargo_metadata::Edition) -> Self {
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
}

/// Lowers Cargo's transport model into the normalized workspace graph used by analysis.
struct CargoMetadataLowerer {
    target_cfg: CfgOptions,
    config: WorkspaceLoweringConfig,
    workspace_members: HashSet<PackageId>,
    dependencies_by_package: HashMap<PackageId, Vec<PackageDependency>>,
    features_by_package: HashMap<PackageId, Vec<String>>,
}

impl CargoMetadataLowerer {
    fn lower(
        metadata: cargo_metadata::Metadata,
        target_cfg: CfgOptions,
        config: WorkspaceLoweringConfig,
    ) -> WorkspaceMetadataResult<WorkspaceMetadata> {
        let workspace_root = canonicalize_path(metadata.workspace_root.as_std_path())
            .map_err(WorkspaceMetadataError::Path)?;
        let lowerer = Self::new(&metadata, target_cfg, config);
        let packages = metadata
            .packages
            .into_iter()
            .map(|package| lowerer.package(package))
            .collect::<WorkspaceMetadataResult<Vec<_>>>()?;

        Ok(WorkspaceMetadata::from_parts(
            workspace_root,
            lowerer.target_cfg,
            packages,
        ))
    }

    fn new(
        metadata: &cargo_metadata::Metadata,
        target_cfg: CfgOptions,
        config: WorkspaceLoweringConfig,
    ) -> Self {
        Self {
            target_cfg,
            config,
            workspace_members: metadata
                .workspace_members
                .iter()
                .map(PackageId::from)
                .collect(),
            dependencies_by_package: metadata
                .resolve
                .as_ref()
                .map(Self::dependencies)
                .unwrap_or_default(),
            features_by_package: metadata
                .resolve
                .as_ref()
                .map(Self::active_features)
                .unwrap_or_default(),
        }
    }

    fn package(&self, package: cargo_metadata::Package) -> WorkspaceMetadataResult<Package> {
        let package_id = PackageId::from(&package.id);
        let mut cfg_options = self.target_cfg.clone();
        for feature in self
            .features_by_package
            .get(&package_id)
            .into_iter()
            .flatten()
        {
            cfg_options.insert_key_value("feature", feature);
        }

        let is_workspace_member = self.workspace_members.contains(&package_id);
        if is_workspace_member && self.config.is_cfg_test_enabled() {
            // `cfg(test)` is an analysis mode for roots the user works on, not a target platform
            // fact or third-party package feature.
            cfg_options.insert_atom("test");
        }
        let raw_manifest_path = package.manifest_path.as_std_path();
        let manifest_path =
            canonicalize_path(raw_manifest_path).map_err(WorkspaceMetadataError::Path)?;
        let raw_package_root = raw_manifest_path
            .parent()
            .expect("Cargo package manifest path should have a parent directory");
        let package_root = manifest_path
            .parent()
            .expect("canonical package manifest path should have a parent directory");
        let targets = package
            .targets
            .iter()
            .map(|target| self.target(target, raw_package_root, package_root, is_workspace_member))
            .collect::<WorkspaceMetadataResult<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();

        Ok(Package {
            id: package_id.clone(),
            name: package.name.to_string(),
            edition: RustEdition::from(package.edition),
            origin: if is_workspace_member {
                PackageOrigin::Workspace
            } else {
                PackageOrigin::Dependency
            },
            source: self.package_source(
                &package_id,
                is_workspace_member,
                package.source.as_ref(),
            )?,
            is_workspace_member,
            manifest_path,
            cfg_options,
            targets,
            dependencies: self
                .dependencies_by_package
                .get(&package_id)
                .cloned()
                .unwrap_or_default(),
        })
    }

    fn package_source(
        &self,
        package: &PackageId,
        is_workspace_member: bool,
        source: Option<&cargo_metadata::Source>,
    ) -> WorkspaceMetadataResult<PackageSource> {
        if is_workspace_member {
            return Ok(PackageSource::Workspace);
        }

        let Some(source) = source else {
            return Ok(PackageSource::Path);
        };
        let source = source.repr.as_str();

        if source.starts_with("path+") {
            Ok(PackageSource::Path)
        } else if source.starts_with("registry+") {
            Ok(PackageSource::Registry)
        } else if source.starts_with("sparse+") {
            Ok(PackageSource::SparseRegistry)
        } else if source.starts_with("git+") {
            Ok(PackageSource::Git)
        } else if source.starts_with("local-registry+") {
            Ok(PackageSource::LocalRegistry)
        } else if source.starts_with("directory+") {
            Ok(PackageSource::Directory)
        } else {
            Err(WorkspaceMetadataError::UnsupportedPackageSource {
                package: package.clone(),
                source: source.to_string(),
            })
        }
    }

    fn target(
        &self,
        target: &cargo_metadata::Target,
        raw_package_root: &Path,
        package_root: &Path,
        is_workspace_member: bool,
    ) -> WorkspaceMetadataResult<Option<Target>> {
        let Some(src_path) = Self::normalize_target_src_path(
            target.src_path.as_std_path(),
            raw_package_root,
            package_root,
            is_workspace_member,
        )?
        else {
            return Ok(None);
        };

        Ok(Some(Target {
            name: target.name.to_string(),
            kind: self.target_kind(target),
            src_path,
        }))
    }

    fn normalize_target_src_path(
        path: &Path,
        raw_package_root: &Path,
        package_root: &Path,
        is_workspace_member: bool,
    ) -> WorkspaceMetadataResult<Option<PathBuf>> {
        match canonicalize_path(path) {
            Ok(path) => Ok(Some(path)),
            Err(error) if error.kind() == io::ErrorKind::NotFound && !is_workspace_member => {
                Ok(None)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // Keep workspace target identity stable across the edit that declares a target and
                // the later edit that materializes its file. Non-workspace targets do not
                // participate in that save flow, so missing ones are filtered out above.
                let relative_path = path
                    .strip_prefix(raw_package_root)
                    .map_err(|_| WorkspaceMetadataError::Path(error))?;
                Ok(Some(package_root.join(relative_path)))
            }
            Err(error) => Err(WorkspaceMetadataError::Path(error)),
        }
    }

    fn target_kind(&self, target: &cargo_metadata::Target) -> TargetKind {
        if target.is_kind(cargo_metadata::TargetKind::Lib) {
            TargetKind::Lib
        } else if target.is_kind(cargo_metadata::TargetKind::Bin) {
            TargetKind::Bin
        } else if target.is_kind(cargo_metadata::TargetKind::Example) {
            TargetKind::Example
        } else if target.is_kind(cargo_metadata::TargetKind::Test) {
            TargetKind::Test
        } else if target.is_kind(cargo_metadata::TargetKind::Bench) {
            TargetKind::Bench
        } else if target.is_kind(cargo_metadata::TargetKind::CustomBuild) {
            TargetKind::CustomBuild
        } else {
            let fallback = target
                .kind
                .first()
                .map(|kind| kind.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            TargetKind::Other(fallback)
        }
    }

    fn dependencies(
        resolve: &cargo_metadata::Resolve,
    ) -> HashMap<PackageId, Vec<PackageDependency>> {
        resolve
            .nodes
            .iter()
            .map(|node| {
                (
                    PackageId::from(&node.id),
                    node.deps.iter().map(Self::dependency).collect::<Vec<_>>(),
                )
            })
            .collect()
    }

    fn dependency(dependency: &cargo_metadata::NodeDep) -> PackageDependency {
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

        PackageDependency::new(
            PackageId::from(&dependency.pkg),
            dependency.name.clone(),
            is_normal,
            is_build,
            is_dev,
        )
    }

    fn active_features(resolve: &cargo_metadata::Resolve) -> HashMap<PackageId, Vec<String>> {
        resolve
            .nodes
            .iter()
            .map(|node| {
                let mut features = node
                    .features
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                features.sort();
                features.dedup();
                (PackageId::from(&node.id), features)
            })
            .collect()
    }
}
