//! Storage-free body vocabulary shared by Body IR and type helpers.
//!
//! These types describe lowered body syntax and ownership facts. Resolved body facts such as
//! expression types stay outside the model crate so `rg_ir_model` does not depend on `rg_ty`.

pub mod expr;
pub mod label;
pub mod owner;
pub mod pat;
pub mod path;
pub mod record;
pub mod source;

pub use self::{
    expr::{ClosureCapture, ClosureKind, ExprAssignOp, ExprBinaryOp, ExprRangeKind, ExprUnaryOp},
    label::LabelData,
    owner::BodyOwner,
    pat::{PatBindingMode, PatMutability, PatRangeKind},
    path::{BodyPath, BodyPathSegment, BodyPathSegmentArgs, BodyPathSegmentKind},
    record::RecordFieldSyntax,
    source::BodySource,
};
