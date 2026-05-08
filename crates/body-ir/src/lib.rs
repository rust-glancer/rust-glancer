mod body;
mod build;
mod cache;
mod cursor;
mod db;
mod expr;
mod ids;
mod item;
mod memsize;
mod pat;
mod path;
mod resolution;
mod resolved;
mod stmt;
mod txn;
mod ty;

#[cfg(test)]
mod tests;

pub use self::{
    body::{
        BodyData, BodyIrStats, BodySource, PackageBodies, ScopeData, TargetBodies,
        TargetBodiesStatus,
    },
    cache::BodyIrPackageBundle,
    cursor::{BodyCursorCandidate, DotReceiver},
    db::BodyIrDb,
    expr::{ExprData, ExprKind, LiteralKind},
    ids::{
        BindingId, BodyFieldRef, BodyFunctionId, BodyFunctionRef, BodyId, BodyImplId, BodyItemId,
        BodyItemRef, BodyRef, ExprId, PatId, ScopeId,
    },
    item::{
        BodyFieldData, BodyFunctionData, BodyFunctionOwner, BodyImplData, BodyItemData,
        BodyItemKind,
    },
    pat::{PatData, PatKind, RecordPatField},
    path::BodyPath,
    resolved::{BodyResolution, BodyTypePathResolution, ResolvedFieldRef, ResolvedFunctionRef},
    stmt::{BindingData, BindingKind, StmtData, StmtKind},
    txn::BodyIrReadTxn,
    ty::{BodyGenericArg, BodyLocalNominalTy, BodyNominalTy, BodyTy},
};

/// Package-set selector for eager body lowering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum BodyIrPackageScope {
    #[default]
    WorkspacePackages,
    AllPackages,
}

/// Controls which packages get function-body lowering during eager Body IR construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
