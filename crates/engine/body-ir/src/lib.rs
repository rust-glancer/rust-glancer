mod build;
mod cursor;
mod ir;
mod resolution;
mod store;
mod view;
mod walk;

use rg_def_map::PackageSlot;
use rg_parse::FileId;

pub use rg_item_tree::FieldKey;

#[cfg(test)]
mod tests;

pub use self::{
    cursor::{
        BodyCursorCandidate, BodyUnqualifiedCompletionCandidate, DotCompletionSite,
        PathCompletionNamespace, PathCompletionSite, RecordFieldCompletionSite,
        UnqualifiedCompletionNamespace, UnqualifiedCompletionSite,
    },
    ir::{
        BindingData, BindingId, BindingKind, BodyBindingRef, BodyData, BodyDeclarationRef,
        BodyEnumVariantData, BodyEnumVariantRef, BodyFieldData, BodyFieldRef, BodyFunctionData,
        BodyFunctionId, BodyFunctionOwner, BodyFunctionRef, BodyId, BodyImplData, BodyImplId,
        BodyImplRef, BodyIrStats, BodyItemData, BodyItemDeclaration, BodyItemId, BodyItemKind,
        BodyItemOwner, BodyItemRef, BodyPath, BodyRef, BodySelfParamKind, BodySource,
        BodyValueItemData, BodyValueItemDeclaration, BodyValueItemId, BodyValueItemKind,
        BodyValueItemOwner, BodyValueItemRef, ClosureCapture, ClosureKind, ClosureParamData,
        ExprAssignOp, ExprBinaryOp, ExprBlockKind, ExprData, ExprId, ExprKind, ExprRangeKind,
        ExprUnaryOp, LabelData, LiteralKind, PackageBodies, PatBindingMode, PatData, PatId,
        PatKind, PatMutability, PatRangeKind, RecordExprField, RecordExprSpread, RecordPatField,
        ScopeData, ScopeId, StmtData, StmtKind, TargetBodies, TargetBodiesStatus,
    },
    store::{BodyIrDb, BodyIrReadTxn},
};

// TODO: Shouldn't be exposed normally; remove after analysis owns resolver projection.
pub use self::ir::{BodyResolution, BodyTypePathResolution};

// TODO: Shouldn't be exposed normally; remove after the type model boundary is extracted.
pub use self::{
    ir::{BodyGenericArg, BodyLocalNominalTy, BodyNominalTy, BodyTy, BodyTyExt, BodyTyRepr},
    resolution::{
        BodyAutoderef, BodyAutoderefCandidate, BodyAutoderefCandidates, BodyAutoderefMode,
    },
};

// TODO: Shouldn't be exposed normally; remove after body-local declaration projection moves out.
pub use self::view::BodyDeclarationView;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, rg_memsize::MemorySize)]
#[memsize(leaf)]
enum BodyIrPackageScope {
    #[default]
    WorkspacePackages,
    AllPackages,
}

/// Controls which packages get function-body lowering during eager Body IR construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, rg_memsize::MemorySize)]
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
