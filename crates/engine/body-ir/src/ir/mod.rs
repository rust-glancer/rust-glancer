//! Body IR domain model.

pub(crate) mod body;
pub(crate) mod body_map;
pub(crate) mod expr;
pub(crate) mod pat;
pub(crate) mod path;
pub(crate) mod resolved;
pub(crate) mod stmt;

pub use self::{
    body::{
        BodyData, BodyIrStats, BodySource, PackageBodies, ScopeData, TargetBodies,
        TargetBodiesStatus,
    },
    body_map::BodySourceItems,
    expr::{
        ClosureCapture, ClosureKind, ClosureParamData, ExprAssignOp, ExprBinaryOp, ExprBlockKind,
        ExprData, ExprKind, ExprRangeKind, ExprUnaryOp, ExprWrapperKind, LabelData, LiteralKind,
        MatchArmData, RecordExprField, RecordExprSpread,
    },
    pat::{PatBindingMode, PatData, PatKind, PatMutability, PatRangeKind, RecordPatField},
    path::BodyPath,
    resolved::BodyResolution,
    stmt::{BindingData, BindingKind, BodySelfParamKind, StmtData, StmtKind},
};

pub(crate) use self::body::BodyBuilder;
