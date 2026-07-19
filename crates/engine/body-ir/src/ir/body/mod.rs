//! Structural body vocabulary owned by Body IR.
//!
//! These types describe lowered body syntax and ownership facts. Resolved paths and inferred
//! types stay in the aligned fact sidecar rather than being folded into this syntax-shaped data.

pub mod binding;
pub mod data;
pub mod expr;
pub mod label;
pub mod macro_call;
pub mod owner;
pub mod pat;
pub mod path;
pub mod record;
pub mod scope;
pub mod source_items;
pub mod stmt;

pub use self::{
    binding::{BindingData, BindingKind},
    data::{BodyData, FunctionParamData},
    expr::{
        ClosureCapture, ClosureKind, ClosureParamData, ExprAssignOp, ExprBlockKind, ExprData,
        ExprKind, ExprRangeKind, ExprWrapperKind, MatchArmData, RecordExprField, RecordExprSpread,
    },
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
};

pub use rg_ir_model::{BodySource, BuiltinMacroExprKind, ExprBinaryOp, ExprUnaryOp, LiteralKind};
