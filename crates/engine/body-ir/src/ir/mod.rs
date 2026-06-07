//! Body IR domain model.

pub(crate) mod body;
pub(crate) mod resolved;
pub(crate) mod source_items;

pub use rg_ir_model::{
    BindingData, BindingKind, BodyOwner, BodyPath, BodySelfParamKind, BodySource, ClosureCapture,
    ClosureKind, ClosureParamData, ExprAssignOp, ExprBinaryOp, ExprBlockKind, ExprData, ExprKind,
    ExprRangeKind, ExprUnaryOp, ExprWrapperKind, LabelData, LiteralKind, MatchArmData,
    PatBindingMode, PatData, PatKind, PatMutability, PatRangeKind, RecordExprField,
    RecordExprSpread, RecordFieldSyntax, RecordPatField, ScopeData, StmtData, StmtKind,
};

pub use self::{
    body::ResolvedBodyData,
    resolved::{BindingFacts, BodyResolution, ExprFacts},
    source_items::BodySourceItems,
};

pub(crate) use self::body::{BodyBuilder, PendingBindingResolution};
