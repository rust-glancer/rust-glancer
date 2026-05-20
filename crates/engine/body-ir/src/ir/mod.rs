//! Body IR domain model.

pub(crate) mod body;
pub(crate) mod expr;
pub(crate) mod ids;
pub(crate) mod item;
pub(crate) mod pat;
pub(crate) mod path;
pub(crate) mod resolved;
pub(crate) mod stmt;
pub(crate) mod ty;

pub use self::{
    body::{
        BodyData, BodyIrStats, BodySource, PackageBodies, ScopeData, TargetBodies,
        TargetBodiesStatus,
    },
    expr::{
        ClosureCapture, ClosureKind, ClosureParamData, ExprAssignOp, ExprBinaryOp, ExprBlockKind,
        ExprData, ExprKind, ExprRangeKind, ExprUnaryOp, ExprWrapperKind, LabelData, LiteralKind,
        MatchArmData, RecordExprField, RecordExprSpread,
    },
    ids::{
        BindingId, BodyEnumVariantRef, BodyFieldRef, BodyFunctionId, BodyFunctionRef, BodyId,
        BodyImplId, BodyItemId, BodyItemRef, BodyRef, BodyValueItemId, BodyValueItemRef, ExprId,
        PatId, ScopeId, StmtId,
    },
    item::{
        BodyEnumVariantData, BodyFieldData, BodyFunctionData, BodyFunctionOwner, BodyImplData,
        BodyItemData, BodyItemDeclaration, BodyItemKind, BodyItemOwner, BodyValueItemData,
        BodyValueItemDeclaration, BodyValueItemKind, BodyValueItemOwner,
    },
    pat::{PatBindingMode, PatData, PatKind, PatMutability, PatRangeKind, RecordPatField},
    path::BodyPath,
    resolved::{
        BodyResolution, BodyTypePathResolution, ResolvedEnumVariantRef, ResolvedFieldRef,
        ResolvedFunctionRef,
    },
    stmt::{BindingData, BindingKind, StmtData, StmtKind},
    ty::{BodyGenericArg, BodyLocalNominalTy, BodyNominalTy, BodyTy},
};

pub(crate) use self::body::BodyBuilder;
