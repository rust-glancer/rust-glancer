use rg_std::MemorySize;
mod build;
mod ir;
mod profile;
mod resolution;
mod store;
#[doc(hidden)]
pub mod testonly;

use rg_def_map::PackageSlot;
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

/// One package-local source file whose function bodies should be lowered during a partial rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BodyIrFile {
    pub package: PackageSlot,
    pub file: FileId,
}

impl BodyIrFile {
    pub fn new(package: PackageSlot, file: FileId) -> Self {
        Self { package, file }
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

/// Controls which packages get function-body lowering during eager Body IR construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, MemorySize)]
pub struct BodyIrBuildPolicy {
    package_scope: BodyIrPackageScope,
}

impl BodyIrBuildPolicy {
    /// Lowers only workspace packages.
    pub fn workspace_packages() -> Self {
        Self {
            package_scope: BodyIrPackageScope::WorkspacePackages,
        }
    }

    /// Lowers every parsed package, including dependencies and sysroot crates.
    pub fn all_packages() -> Self {
        Self {
            package_scope: BodyIrPackageScope::AllPackages,
        }
    }

    /// Returns whether eager body lowering should produce bodies for this parsed package.
    pub fn should_lower_package(&self, package: &rg_parse::Package) -> bool {
        match self.package_scope {
            BodyIrPackageScope::WorkspacePackages => package.is_workspace_member(),
            BodyIrPackageScope::AllPackages => true,
        }
    }
}
