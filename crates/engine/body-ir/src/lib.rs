//! Analysis of code inside function, const, and static bodies.
//!
//! `build` lowers syntax and prepares local declarations before `resolution` derives semantic
//! facts. `body` owns the frozen structure, its facts, and the views that read them together.
//! `store` retains saved body packages and the current bodies prepared for an editor request.

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
        BodyIrBuildPolicy, BodyIrBuildProgress, BodyIrBuildStage, BodyIrBuilder, BodyIrFile,
        CurrentSourceBuildCheckpoint, CurrentSourceBuildSummary, CurrentSourceBuilder,
        CurrentSourceSelection, CurrentSourceUnavailable,
    },
    profile::profile_descriptors,
};

#[cfg(test)]
mod tests;

pub use self::{
    body::{
        BindingData, BindingKind, BodyAssociatedPathPrefix, BodyData, BodyFacts, BodyMacroCallData,
        BodyOwner, BodyPath, BodyPathSegment, BodyPathSegmentArgs, BodyPathSegmentKind, BodySource,
        BodySourceItem, BodySourceItems, BodyView, BuiltinMacroExprKind, CallFacts, ClosureCapture,
        ClosureKind, ClosureParamData, ExprAssignOp, ExprBinaryOp, ExprBlockKind, ExprData,
        ExprFacts, ExprKind, ExprRangeKind, ExprUnaryOp, ExprWrapperKind, FunctionParamData,
        LabelData, LiteralKind, MatchArmData, PatBindingMode, PatData, PatKind, PatRangeKind,
        RecordExprField, RecordExprSpread, RecordFieldSyntax, RecordPatField, ScopeData, StmtData,
        StmtKind,
    },
    resolution::{BodyMethodQuery, BodyResolutionContext, BodyTypePathQuery, BodyValuePathQuery},
    store::{
        BodyFileEntry, BodyFileShard, BodyIrDb, BodyIrLoader, BodyIrReadTxn, BodyIrStats,
        BodyLocalItems, CrateBodies, CrateBodiesCoverage, CrateBodiesManifest, CrateBodiesStatus,
        CurrentSourceStore, LoadBodyIr, PackageBodies, PackageBodiesCoverage,
        PackageBodiesManifest,
    },
};
