use super::{package::PackageId, target::TargetKind};
use rg_std::MemorySize;

/// One dependency edge after Cargo resolution.
#[derive(Debug, Clone, PartialEq, Eq, MemorySize)]
pub struct PackageDependency {
    package: PackageId,
    name: String,
    is_normal: bool,
    is_build: bool,
    is_dev: bool,
}

impl PackageDependency {
    pub(crate) fn new(
        package: PackageId,
        name: impl Into<String>,
        is_normal: bool,
        is_build: bool,
        is_dev: bool,
    ) -> Self {
        Self {
            package,
            name: name.into(),
            is_normal,
            is_build,
            is_dev,
        }
    }

    pub(crate) fn normal(package: PackageId, name: impl Into<String>) -> Self {
        Self {
            package,
            name: name.into(),
            is_normal: true,
            is_build: false,
            is_dev: false,
        }
    }

    pub(crate) fn for_all_targets(package: PackageId, name: impl Into<String>) -> Self {
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
