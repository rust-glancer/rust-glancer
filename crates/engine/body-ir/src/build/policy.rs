//! Package, target, and file selection for saved body builds.

use rg_ir_model::{CrateRef, FileId};
use rg_std::MemorySize;

/// One semantic crate interpretation of a source file selected for Body IR lowering.
///
/// A source file can participate in more than one Cargo target. Keeping the crate identity beside
/// the file prevents an exact request for one target from also lowering a sibling target that reads
/// the same file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BodyIrFile {
    pub crate_ref: CrateRef,
    pub file: FileId,
}

impl BodyIrFile {
    pub fn new(crate_ref: CrateRef, file: FileId) -> Self {
        Self { crate_ref, file }
    }
}

/// Package-set selector for eager body lowering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, MemorySize)]
#[memsize(leaf)]
enum BodyIrPackageScope {
    #[default]
    WorkspacePackages,
    AllPackages,
}

/// Target-set selector for eager body lowering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, MemorySize)]
#[memsize(leaf)]
enum BodyIrTargetScope {
    #[default]
    PrimaryTargets,
    AllTargets,
}

/// Controls which packages and Cargo targets get bodies during eager Body IR construction.
///
/// Interactive indexing uses primary workspace targets so target-heavy test suites do not inflate
/// retained memory. Broader operations can select every package and target explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, MemorySize)]
pub struct BodyIrBuildPolicy {
    package_scope: BodyIrPackageScope,
    target_scope: BodyIrTargetScope,
}

impl BodyIrBuildPolicy {
    /// Lower every target from every parsed package, including dependencies and sysroot crates.
    pub fn all_packages() -> Self {
        Self {
            package_scope: BodyIrPackageScope::AllPackages,
            target_scope: BodyIrTargetScope::AllTargets,
        }
    }

    /// Returns whether eager body lowering should produce bodies for this parsed package.
    pub fn should_lower_package(&self, package: &rg_parse::Package) -> bool {
        match self.package_scope {
            BodyIrPackageScope::WorkspacePackages => package.is_workspace_member(),
            BodyIrPackageScope::AllPackages => true,
        }
    }

    /// Returns whether eager body lowering should produce bodies for this Cargo target.
    pub fn should_lower_target(
        &self,
        package: &rg_parse::Package,
        target: &rg_parse::CargoTarget,
    ) -> bool {
        self.should_lower_package(package)
            && match self.target_scope {
                BodyIrTargetScope::PrimaryTargets => target.kind.is_primary_analysis_target(),
                BodyIrTargetScope::AllTargets => true,
            }
    }
}
