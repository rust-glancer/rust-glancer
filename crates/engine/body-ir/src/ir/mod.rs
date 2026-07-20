//! Structural body IR and the semantic facts derived from it.
//!
//! `BodyData` is the frozen syntax-shaped body owned here. Resolution writes a separate
//! `BodyFacts` sidecar, and consumers normally read the aligned pair through `BodyView`.
//! The inference-only projection remains crate-private so partially resolved state does not look
//! like a finalized body. Mutable build state lives under `build::lower`, outside this read model.

pub(crate) mod body;
pub(crate) mod resolved;
pub(crate) mod view;

pub use self::{
    body::{
        BindingData, BindingKind, BodyAssociatedPathPrefix, BodyData, BodyMacroCallData, BodyOwner,
        BodyPath, BodyPathSegment, BodyPathSegmentArgs, BodyPathSegmentKind, BodySource,
        BodySourceItem, BodySourceItems, BuiltinMacroExprKind, ClosureCapture, ClosureKind,
        ClosureParamData, ExprAssignOp, ExprBinaryOp, ExprBlockKind, ExprData, ExprKind,
        ExprRangeKind, ExprUnaryOp, ExprWrapperKind, FunctionParamData, LabelData, LiteralKind,
        MatchArmData, PatBindingMode, PatData, PatKind, PatRangeKind, RecordExprField,
        RecordExprSpread, RecordFieldSyntax, RecordPatField, ScopeData, StmtData, StmtKind,
    },
    resolved::{BindingFacts, BodyFacts, CallFacts, ExprFacts},
    view::BodyView,
};

pub(crate) use self::view::BodyQueryView;
