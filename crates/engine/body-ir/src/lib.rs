//! Analysis of code inside function, const, and static bodies.
//!
//! `build` lowers syntax and prepares local declarations before `resolution` derives semantic
//! facts. `body` owns the frozen structure, its facts, and the views that read them together.
//! `store` reads saved owner results and the current bodies prepared for an editor request.
//!
//! Preparation freezes an outer owner and all its nested bodies together. Its semantic data and
//! source tables are separate query outputs: moving code can update locations while inference
//! reuses the unchanged data. `BodyView` borrows structure, facts, and locations for one operation.
//! Editor requests use the same preparation and inference, reusing saved facts only when the
//! complete prepared owner matches.

mod body;
mod build;
mod profile;
mod resolution;
mod store;
#[doc(hidden)]
pub mod testonly;

pub use rg_ir_model::FieldKey;

pub(crate) use self::store::CurrentBody;
pub use self::{
    build::{
        CurrentSourceBuildCheckpoint, CurrentSourceBuildSummary, CurrentSourceBuilder,
        CurrentSourceSelection, CurrentSourceUnavailable, PreparedBody, infer_saved_body,
        prepare_saved_body, saved_body,
    },
    profile::profile_descriptors,
};

#[cfg(test)]
mod tests;

pub use self::{
    body::{
        BindingData, BindingKind, BodyAssociatedPathPrefix, BodyData, BodyFacts, BodyMacroCallData,
        BodyOwner, BodyPath, BodyPathSegment, BodyPathSegmentArgs, BodyPathSegmentKind, BodySource,
        BodySourceItem, BodySourceItems, BodySourceStore, BodyView, BuiltinMacroExprKind,
        CallFacts, ClosureCapture, ClosureKind, ClosureParamData, ExprAssignOp, ExprBinaryOp,
        ExprBlockKind, ExprData, ExprFacts, ExprKind, ExprRangeKind, ExprUnaryOp, ExprWrapperKind,
        FunctionParamData, LabelData, LiteralKind, MatchArmData, PatBindingMode, PatData, PatKind,
        PatRangeKind, RecordExprField, RecordExprSpread, RecordFieldSyntax, RecordPatField,
        ScopeData, StmtData, StmtKind,
    },
    resolution::{
        BodyFieldQuery, BodyMethodQuery, BodyResolutionContext, BodyTypePathQuery,
        BodyValuePathQuery,
    },
    store::{
        BodyFactsStore, BodyLocalItems, BodyReader, BodyStore, CurrentSourceStore,
        DeclarationSourceView, PreparedBodyStore,
    },
};
