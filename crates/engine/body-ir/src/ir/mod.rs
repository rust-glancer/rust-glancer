//! Body IR domain model.

pub(crate) mod body;
pub(crate) mod expr;
pub(crate) mod pat;
pub(crate) mod path;
pub(crate) mod record;
pub(crate) mod resolved;
pub(crate) mod source_items;
pub(crate) mod stmt;

pub use self::{
    body::{
        BodyData, BodyIrStats, BodyLocalItems, BodyOwner, BodySource, PackageBodies, ScopeData,
        TargetBodies, TargetBodiesStatus,
    },
    expr::{
        ClosureCapture, ClosureKind, ClosureParamData, ExprAssignOp, ExprBinaryOp, ExprBlockKind,
        ExprData, ExprKind, ExprRangeKind, ExprUnaryOp, ExprWrapperKind, LabelData, LiteralKind,
        MatchArmData, RecordExprField, RecordExprSpread,
    },
    pat::{PatBindingMode, PatData, PatKind, PatMutability, PatRangeKind, RecordPatField},
    path::BodyPath,
    record::RecordFieldSyntax,
    resolved::BodyResolution,
    source_items::BodySourceItems,
    stmt::{BindingData, BindingKind, BodySelfParamKind, StmtData, StmtKind},
};

pub(crate) use self::body::BodyBuilder;
