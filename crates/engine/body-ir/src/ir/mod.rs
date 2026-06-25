//! Body IR domain model.

pub(crate) mod body;
pub(crate) mod resolved;

pub use rg_ir_model::{
    BindingData, BindingKind, BodyAssociatedPathPrefix, BodyOwner, BodyPath, BodySelfParamKind,
    BodySource, BodySourceItems, BuiltinMacroExprKind, ClosureCapture, ClosureKind,
    ClosureParamData, ExprAssignOp, ExprBinaryOp, ExprBlockKind, ExprData, ExprKind, ExprRangeKind,
    ExprUnaryOp, ExprWrapperKind, FunctionParamData, LabelData, LiteralKind, MatchArmData,
    PatBindingMode, PatData, PatKind, PatRangeKind, RecordExprField, RecordExprSpread,
    RecordFieldSyntax, RecordPatField, ScopeData, StmtData, StmtKind,
};

pub use self::{
    body::ResolvedBodyData,
    resolved::{BindingFacts, ExprFacts},
};

pub(crate) use self::body::{BodyBuilder, PendingBindingResolution};
