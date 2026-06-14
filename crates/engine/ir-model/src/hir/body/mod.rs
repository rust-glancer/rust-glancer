//! Storage-free body vocabulary shared by Body IR and type helpers.
//!
//! These types describe lowered body syntax and ownership facts. Resolved body facts such as
//! expression types stay outside the model crate so `rg_ir_model` does not depend on `rg_ty`.

pub mod binding;
pub mod data;
pub mod expr;
pub mod label;
pub mod literal;
pub mod owner;
pub mod pat;
pub mod path;
pub mod record;
pub mod scope;
pub mod source;
pub mod source_items;
pub mod stmt;

pub use self::{
    binding::{BindingData, BindingKind, BodySelfParamKind},
    data::{BodyData, FunctionParamData},
    expr::{
        ClosureCapture, ClosureKind, ClosureParamData, ExprAssignOp, ExprBinaryOp, ExprBlockKind,
        ExprData, ExprKind, ExprRangeKind, ExprUnaryOp, ExprWrapperKind, MatchArmData,
        RecordExprField, RecordExprSpread,
    },
    label::LabelData,
    literal::LiteralKind,
    owner::BodyOwner,
    pat::{PatBindingMode, PatData, PatKind, PatMutability, PatRangeKind, RecordPatField},
    path::{BodyPath, BodyPathSegment, BodyPathSegmentArgs, BodyPathSegmentKind},
    record::RecordFieldSyntax,
    scope::ScopeData,
    source::BodySource,
    source_items::BodySourceItems,
    stmt::{StmtData, StmtKind},
};
