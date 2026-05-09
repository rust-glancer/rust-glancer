use rg_item_tree::FieldKey;
use rg_parse::Span;
use rg_text::Name;

use crate::{
    body::BodySource,
    ids::{ExprId, PatId, ScopeId, StmtId},
    path::BodyPath,
    resolved::BodyResolution,
    ty::BodyTy,
};

/// One lowered expression.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ExprData {
    pub source: BodySource,
    pub scope: ScopeId,
    /// Number of body-wide bindings that were visible at this expression's source location.
    ///
    /// Scope data is frozen after lowering, so the resolver needs this boundary to avoid letting a
    /// later `let x` shadow an earlier use of `x`.
    pub visible_bindings: usize,
    pub kind: ExprKind,
    pub resolution: BodyResolution,
    pub ty: BodyTy,
}

impl ExprData {
    pub(crate) fn shrink_to_fit(&mut self) {
        self.kind.shrink_to_fit();
        self.resolution.shrink_to_fit();
        self.ty.shrink_to_fit();
    }
}

/// Expression forms that the first Body IR pass understands.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ExprKind {
    Block {
        scope: ScopeId,
        statements: Vec<StmtId>,
        tail: Option<ExprId>,
    },
    Path {
        path: BodyPath,
    },
    Call {
        callee: Option<ExprId>,
        args: Vec<ExprId>,
    },
    Match {
        scrutinee: Option<ExprId>,
        arms: Vec<MatchArmData>,
    },
    MethodCall {
        receiver: Option<ExprId>,
        dot_span: Option<Span>,
        method_name: Name,
        method_name_span: Option<Span>,
        args: Vec<ExprId>,
    },
    Field {
        base: Option<ExprId>,
        dot_span: Option<Span>,
        field: Option<FieldKey>,
        field_span: Option<Span>,
    },
    Wrapper {
        kind: ExprWrapperKind,
        inner: Option<ExprId>,
    },
    Literal {
        kind: LiteralKind,
    },
    Unknown {
        children: Vec<ExprId>,
    },
}

/// Transparent or nearly-transparent expression wrapper understood by cheap type normalization.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub enum ExprWrapperKind {
    #[display("paren")]
    Paren,
    #[display("ref")]
    Ref,
    #[display("await")]
    Await,
    #[display("try")]
    Try,
    #[display("return")]
    Return,
}

/// One match arm with its pattern scope and lowered arm expression.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MatchArmData {
    pub pat: Option<PatId>,
    pub scope: ScopeId,
    pub expr: Option<ExprId>,
}

/// Literal category used for display and future cheap inference.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub enum LiteralKind {
    #[display("bool")]
    Bool,
    #[display("char")]
    Char,
    #[display("float")]
    Float,
    #[display("int")]
    Int,
    #[display("string")]
    String,
    #[display("unknown")]
    Unknown,
}

impl ExprKind {
    fn shrink_to_fit(&mut self) {
        match self {
            Self::Block {
                statements, tail, ..
            } => {
                statements.shrink_to_fit();
                let _ = tail;
            }
            Self::Path { path } => path.shrink_to_fit(),
            Self::Call { callee, args } => {
                let _ = callee;
                args.shrink_to_fit();
            }
            Self::Match { scrutinee, arms } => {
                let _ = scrutinee;
                arms.shrink_to_fit();
            }
            Self::MethodCall {
                receiver,
                method_name,
                args,
                ..
            } => {
                let _ = receiver;
                method_name.shrink_to_fit();
                args.shrink_to_fit();
            }
            Self::Field { field, .. } => {
                if let Some(field) = field {
                    field.shrink_to_fit();
                }
            }
            Self::Wrapper { .. } | Self::Literal { .. } => {}
            Self::Unknown { children } => children.shrink_to_fit(),
        }
    }
}
