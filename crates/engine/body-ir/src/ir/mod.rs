//! Body IR domain model.

pub(crate) mod body;
pub(crate) mod resolved;

pub use rg_ir_model::{
    BindingData, BindingKind, BodyAssociatedPathPrefix, BodyData, BodyOwner, BodyPath, BodySource,
    BodySourceItems, BuiltinMacroExprKind, ClosureCapture, ClosureKind, ClosureParamData,
    ExprAssignOp, ExprBinaryOp, ExprBlockKind, ExprData, ExprKind, ExprRangeKind, ExprUnaryOp,
    ExprWrapperKind, FunctionParamData, LabelData, LiteralKind, MatchArmData, PatBindingMode,
    PatData, PatKind, PatRangeKind, RecordExprField, RecordExprSpread, RecordFieldSyntax,
    RecordPatField, ScopeData, StmtData, StmtKind,
};

pub use self::{
    body::BodyView,
    resolved::{BindingFacts, BodyFacts, ExprFacts},
};

pub(crate) use self::body::{BodyBuilder, LoweredBodyData, PendingBindingResolution};
