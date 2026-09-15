//! Structural body IR and the semantic facts derived from it.
//!
//! `BodyData` is the frozen syntax-shaped body owned here. Resolution writes a separate
//! `BodyFacts` sidecar, and consumers normally read the aligned pair through `BodyView`.
//! The inference-only projection remains crate-private so partially resolved state does not look
//! like a finalized body. Mutable build state lives under `build::lower`, outside this read model.

mod binding;
mod data;
mod expr;
pub(crate) mod facts;
mod label;
mod macro_call;
mod owner;
mod pat;
mod path;
mod record;
mod scope;
mod source_items;
mod stmt;
mod view;

pub use self::{
    binding::{BindingData, BindingKind},
    data::{BodyData, FunctionParamData},
    expr::{
        ClosureCapture, ClosureKind, ClosureParamData, ExprAssignOp, ExprBlockKind, ExprData,
        ExprKind, ExprRangeKind, ExprWrapperKind, MatchArmData, RecordExprField, RecordExprSpread,
    },
    facts::{BindingFacts, BodyFacts, CallFacts, ExprFacts},
    label::LabelData,
    macro_call::BodyMacroCallData,
    owner::BodyOwner,
    pat::{PatBindingMode, PatData, PatKind, PatRangeKind, RecordPatField},
    path::{
        BodyAssociatedPathPrefix, BodyPath, BodyPathSegment, BodyPathSegmentArgs,
        BodyPathSegmentKind,
    },
    record::RecordFieldSyntax,
    scope::ScopeData,
    source_items::{BodySourceItem, BodySourceItems},
    stmt::{StmtData, StmtKind},
    view::BodyView,
};

pub use rg_ir_model::{BodySource, BuiltinMacroExprKind, ExprBinaryOp, ExprUnaryOp, LiteralKind};

pub(crate) use self::view::BodyQueryView;
