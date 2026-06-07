//! Body IR domain model.

pub(crate) mod body;
pub(crate) mod expr;
pub(crate) mod pat;
pub(crate) mod path;
pub(crate) mod resolved;
pub(crate) mod source_items;
pub(crate) mod stmt;

pub use rg_ir_model::{
    BodyOwner, BodySource, ClosureCapture, ClosureKind, ExprAssignOp, ExprBinaryOp, ExprRangeKind,
    ExprUnaryOp, LabelData, PatBindingMode, PatMutability, PatRangeKind, RecordFieldSyntax,
};

pub use self::{
    body::{
        BodyData, BodyIrStats, BodyLocalItems, PackageBodies, ScopeData, TargetBodies,
        TargetBodiesStatus,
    },
    expr::{
        ClosureParamData, ExprBlockKind, ExprData, ExprKind, ExprWrapperKind, LiteralKind,
        MatchArmData, RecordExprField, RecordExprSpread,
    },
    pat::{PatData, PatKind, RecordPatField},
    path::BodyPath,
    resolved::BodyResolution,
    source_items::BodySourceItems,
    stmt::{BindingData, BindingKind, BodySelfParamKind, StmtData, StmtKind},
};

pub(crate) use self::body::BodyBuilder;
