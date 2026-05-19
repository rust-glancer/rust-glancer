//! Body IR domain model.

pub(crate) mod body;
pub(crate) mod expr;
pub(crate) mod ids;
pub(crate) mod item;
pub(crate) mod pat;
pub(crate) mod path;
pub(crate) mod resolved;
pub(crate) mod stmt;
pub(crate) mod ty;

pub use self::{
    body::{
        BodyData, BodyIrStats, BodySource, PackageBodies, ScopeData, TargetBodies,
        TargetBodiesStatus,
    },
    expr::{
        ClosureCapture, ClosureKind, ClosureParamData, ExprAssignOp, ExprBinaryOp, ExprData,
        ExprKind, ExprRangeKind, ExprUnaryOp, ExprWrapperKind, LabelData, LiteralKind,
        MatchArmData, RecordExprField,
    },
    ids::{
        BindingId, BodyFieldRef, BodyFunctionId, BodyFunctionRef, BodyId, BodyImplId, BodyItemId,
        BodyItemRef, BodyRef, ExprId, PatId, ScopeId, StmtId,
    },
    item::{
        BodyFieldData, BodyFunctionData, BodyFunctionOwner, BodyImplData, BodyItemData,
        BodyItemKind,
    },
    pat::{PatBindingMode, PatData, PatKind, PatMutability, PatRangeKind, RecordPatField},
    path::BodyPath,
    resolved::{BodyResolution, BodyTypePathResolution, ResolvedFieldRef, ResolvedFunctionRef},
    stmt::{BindingData, BindingKind, StmtData, StmtKind},
    ty::{BodyGenericArg, BodyLocalNominalTy, BodyNominalTy, BodyTy},
};

pub(crate) use self::body::BodyBuilder;
