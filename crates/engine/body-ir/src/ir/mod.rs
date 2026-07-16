//! Structural body IR and the semantic facts derived from it.
//!
//! `BodyData` is the frozen syntax-shaped body imported from `rg_ir_model`. Resolution writes a
//! separate `BodyFacts` sidecar, and consumers normally read the aligned pair through `BodyView`.
//! Build-only and inference-only projections remain crate-private so partially resolved state does
//! not look like a finalized body.

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
    resolved::{BindingFacts, BodyFacts, CallFacts, ExprFacts},
};

pub(crate) use self::body::{
    BodyBuilder, BodyQueryView, LoweredBodyData, PendingBindingResolution,
};
