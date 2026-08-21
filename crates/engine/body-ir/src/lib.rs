use rg_std::MemorySize;
mod build;
mod ir;
mod profile;
mod resolution;
mod store;
#[doc(hidden)]
pub mod testonly;

use rg_ir_model::CrateRef;
use rg_parse::FileId;

pub use self::profile::profile_descriptors;
pub use rg_ir_model::FieldKey;

pub use self::build::{
    CurrentBodyBuildCheckpoint, CurrentBodyBuildOutcome, CurrentBodyBuilder, CurrentBodySelection,
    CurrentBodyUnavailable,
};

#[cfg(test)]
mod tests;

pub use self::{
    ir::{
        BindingData, BindingFacts, BindingKind, BodyAssociatedPathPrefix, BodyData, BodyFacts,
        BodyMacroCallData, BodyOwner, BodyPath, BodyPathSegment, BodyPathSegmentArgs,
        BodyPathSegmentKind, BodySource, BodySourceItem, BodySourceItems, BodyView,
        BuiltinMacroExprKind, CallFacts, ClosureCapture, ClosureKind, ClosureParamData,
        ExprAssignOp, ExprBinaryOp, ExprBlockKind, ExprData, ExprFacts, ExprKind, ExprRangeKind,
        ExprUnaryOp, ExprWrapperKind, FunctionParamData, LabelData, LiteralKind, MatchArmData,
        PatBindingMode, PatData, PatKind, PatRangeKind, RecordExprField, RecordExprSpread,
        RecordFieldSyntax, RecordPatField, ScopeData, StmtData, StmtKind,
    },
    resolution::{BodyMethodQuery, BodyResolutionContext, BodyTypePathQuery, BodyValuePathQuery},
    store::{
        BodyFileEntry, BodyFileShard, BodyIrDb, BodyIrLoader, BodyIrReadTxn, BodyIrStats,
        BodyLocalItems, CrateBodies, CrateBodiesCoverage, CrateBodiesManifest, CrateBodiesStatus,
        CurrentBody, CurrentBodySet, LoadBodyIr, PackageBodies, PackageBodiesManifest,
    },
};

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
    /// Lower primary targets from workspace packages and defer secondary targets until requested.
    pub fn workspace_packages() -> Self {
        Self {
            package_scope: BodyIrPackageScope::WorkspacePackages,
            target_scope: BodyIrTargetScope::PrimaryTargets,
        }
    }

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
