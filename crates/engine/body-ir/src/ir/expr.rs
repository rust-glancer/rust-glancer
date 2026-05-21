use std::fmt;

use rg_item_tree::{FieldKey, TypeRef};
use rg_parse::Span;
use rg_text::Name;

use super::{
    body::BodySource,
    ids::{BindingId, ExprId, PatId, ScopeId, StmtId},
    path::BodyPath,
    resolved::BodyResolution,
    ty::BodyTy,
};

/// One lowered expression.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
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

/// One field written inside a record expression.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct RecordExprField {
    pub key: FieldKey,
    pub key_span: Span,
    pub source_span: Span,
    pub value: Option<ExprId>,
}

/// `..` or `..base` written after record fields.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct RecordExprSpread {
    pub source_span: Span,
    pub expr: Option<ExprId>,
}

/// Block-level execution modifier written before the statement list.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum ExprBlockKind {
    /// `{ ... }`.
    Plain,
    /// `unsafe { ... }`.
    Unsafe,
    /// `const { ... }`.
    Const,
    /// `async { ... }` or `async move { ... }`.
    Async { move_capture: bool },
    /// `try { ... }` or `try bikeshed Type { ... }`.
    Try {
        bikeshed: bool,
        result_ty: Option<TypeRef>,
    },
    /// `gen { ... }` or `gen move { ... }`.
    Gen { move_capture: bool },
    /// `async gen { ... }` or `async gen move { ... }`.
    AsyncGen { move_capture: bool },
}

impl fmt::Display for ExprBlockKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plain => f.write_str("plain"),
            Self::Unsafe => f.write_str("unsafe"),
            Self::Const => f.write_str("const"),
            Self::Async {
                move_capture: false,
            } => f.write_str("async"),
            Self::Async { move_capture: true } => f.write_str("async move"),
            Self::Try {
                bikeshed: false,
                result_ty: None,
            } => f.write_str("try"),
            Self::Try {
                bikeshed: true,
                result_ty: None,
            } => f.write_str("try bikeshed"),
            Self::Try {
                bikeshed: false,
                result_ty: Some(result_ty),
            } => write!(f, "try {result_ty}"),
            Self::Try {
                bikeshed: true,
                result_ty: Some(result_ty),
            } => write!(f, "try bikeshed {result_ty}"),
            Self::Gen {
                move_capture: false,
            } => f.write_str("gen"),
            Self::Gen { move_capture: true } => f.write_str("gen move"),
            Self::AsyncGen {
                move_capture: false,
            } => f.write_str("async gen"),
            Self::AsyncGen { move_capture: true } => f.write_str("async gen move"),
        }
    }
}

impl ExprBlockKind {
    fn shrink_to_fit(&mut self) {
        if let Self::Try {
            result_ty: Some(result_ty),
            ..
        } = self
        {
            result_ty.shrink_to_fit();
        }
    }
}

/// Expression forms that the first Body IR pass understands.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum ExprKind {
    /// `{ ... }`, `async { ... }`, or `'label: { ... }`.
    Block {
        kind: ExprBlockKind,
        label: Option<LabelData>,
        scope: ScopeId,
        statements: Vec<StmtId>,
        tail: Option<ExprId>,
    },
    /// `value`, `module::item`, or another expression path.
    Path { path: BodyPath },
    /// `<callee>(<arg>, ...)`.
    Call {
        callee: Option<ExprId>,
        args: Vec<ExprId>,
    },
    /// `()`, `(<expr>,)`, or `(<expr>, <expr>)`.
    Tuple { fields: Vec<ExprId> },
    /// `[<expr>, ...]`.
    Array { elements: Vec<ExprId> },
    /// `[<expr>; <len>]`.
    RepeatArray {
        initializer: Option<ExprId>,
        repeat: Option<ExprId>,
    },
    /// `<base>[<index>]`.
    Index {
        base: Option<ExprId>,
        index: Option<ExprId>,
    },
    /// `<start>..<end>`, `<start>..=<end>`, `..<end>`, or `<start>..`.
    Range {
        start: Option<ExprId>,
        end: Option<ExprId>,
        kind: Option<ExprRangeKind>,
    },
    /// `<expr> as Type`.
    Cast {
        expr: Option<ExprId>,
        ty: Option<TypeRef>,
    },
    /// `*<expr>`, `!<expr>`, or `-<expr>`.
    Unary {
        op: Option<ExprUnaryOp>,
        expr: Option<ExprId>,
    },
    /// `<lhs> + <rhs>`, `<lhs> == <rhs>`, or another non-assignment binary expression.
    Binary {
        lhs: Option<ExprId>,
        op: Option<ExprBinaryOp>,
        rhs: Option<ExprId>,
    },
    /// `<target> = <value>` or `<target> += <value>`.
    Assign {
        target: Option<ExprId>,
        op: Option<ExprAssignOp>,
        value: Option<ExprId>,
    },
    /// `match <scrutinee> { ... }`.
    Match {
        scrutinee: Option<ExprId>,
        arms: Vec<MatchArmData>,
    },
    /// `if <condition> { ... } else { ... }`.
    If {
        condition: Option<ExprId>,
        then_branch: Option<ExprId>,
        else_branch: Option<ExprId>,
    },
    /// `let <pat> = <expr>` in expression position, such as an `if` condition.
    Let {
        scope: ScopeId,
        pat: Option<PatId>,
        bindings: Vec<BindingId>,
        initializer: Option<ExprId>,
    },
    /// `|params| body`, `move |params| body`, or `async |params| body`.
    Closure {
        scope: ScopeId,
        capture: ClosureCapture,
        kind: ClosureKind,
        params: Vec<ClosureParamData>,
        ret_ty: Option<TypeRef>,
        body: Option<ExprId>,
    },
    /// `loop { ... }` or `'label: loop { ... }`.
    Loop {
        label: Option<LabelData>,
        body: Option<ExprId>,
    },
    /// `while <condition> { ... }` or `'label: while <condition> { ... }`.
    While {
        label: Option<LabelData>,
        condition: Option<ExprId>,
        body: Option<ExprId>,
    },
    /// `for <pat> in <iterable> { ... }` or `'label: for <pat> in <iterable> { ... }`.
    For {
        label: Option<LabelData>,
        scope: ScopeId,
        pat: Option<PatId>,
        bindings: Vec<BindingId>,
        iterable: Option<ExprId>,
        body: Option<ExprId>,
    },
    /// `break`, `break 'label`, or `break <value>`.
    Break {
        label: Option<LabelData>,
        value: Option<ExprId>,
    },
    /// `continue` or `continue 'label`.
    Continue { label: Option<LabelData> },
    /// `<receiver>.<method>(<arg>, ...)`.
    MethodCall {
        receiver: Option<ExprId>,
        dot_span: Option<Span>,
        method_name: Name,
        method_name_span: Option<Span>,
        args: Vec<ExprId>,
    },
    /// `<base>.field` or `<base>.0`.
    Field {
        base: Option<ExprId>,
        dot_span: Option<Span>,
        field: Option<FieldKey>,
        field_span: Option<Span>,
    },
    /// `Path { field, other: <expr>, ..base }` or `Path { .. }`.
    Record {
        path: Option<BodyPath>,
        field_list_span: Option<Span>,
        fields: Vec<RecordExprField>,
        spread: Option<RecordExprSpread>,
    },
    /// Parentheses, reference, await, try, or return syntax around another expression.
    Wrapper {
        kind: ExprWrapperKind,
        inner: Option<ExprId>,
    },
    /// `42`, `"text"`, `true`, or another literal token.
    Literal { kind: LiteralKind },
    /// `_` in expression position.
    Underscore,
    /// `yield` or `yield <value>`.
    Yield { value: Option<ExprId> },
    /// `do yeet` or `do yeet <value>`.
    Yeet { value: Option<ExprId> },
    /// `become <value>`.
    Become { value: Option<ExprId> },
    /// Expression syntax that Body IR does not model directly.
    Unknown { children: Vec<ExprId> },
}

/// Closure capture mode written before the closure parameter list.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    wincode::SchemaRead,
    wincode::SchemaWrite,
)]
pub enum ClosureCapture {
    #[display("inferred")]
    Inferred,
    #[display("move")]
    Move,
}

/// Closure-level execution modifier that affects how its body is interpreted.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    wincode::SchemaRead,
    wincode::SchemaWrite,
)]
pub enum ClosureKind {
    #[display("normal")]
    Normal,
    #[display("async")]
    Async,
}

/// Unary operator written before an expression.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    wincode::SchemaRead,
    wincode::SchemaWrite,
)]
pub enum ExprUnaryOp {
    #[display("*")]
    Deref,
    #[display("!")]
    Not,
    #[display("-")]
    Neg,
}

/// Non-assignment binary operator written between two expressions.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    wincode::SchemaRead,
    wincode::SchemaWrite,
)]
pub enum ExprBinaryOp {
    #[display("||")]
    LogicOr,
    #[display("&&")]
    LogicAnd,
    #[display("==")]
    Eq,
    #[display("!=")]
    NotEq,
    #[display("<")]
    Less,
    #[display("<=")]
    LessEq,
    #[display(">")]
    Greater,
    #[display(">=")]
    GreaterEq,
    #[display("+")]
    Add,
    #[display("*")]
    Mul,
    #[display("-")]
    Sub,
    #[display("/")]
    Div,
    #[display("%")]
    Rem,
    #[display("<<")]
    Shl,
    #[display(">>")]
    Shr,
    #[display("^")]
    BitXor,
    #[display("|")]
    BitOr,
    #[display("&")]
    BitAnd,
}

/// Assignment operator written between a target expression and a value expression.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    wincode::SchemaRead,
    wincode::SchemaWrite,
)]
pub enum ExprAssignOp {
    #[display("=")]
    Assign,
    #[display("+=")]
    Add,
    #[display("*=")]
    Mul,
    #[display("-=")]
    Sub,
    #[display("/=")]
    Div,
    #[display("%=")]
    Rem,
    #[display("<<=")]
    Shl,
    #[display(">>=")]
    Shr,
    #[display("^=")]
    BitXor,
    #[display("|=")]
    BitOr,
    #[display("&=")]
    BitAnd,
}

/// Range operator written between optional range bounds.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    wincode::SchemaRead,
    wincode::SchemaWrite,
)]
pub enum ExprRangeKind {
    /// `..`.
    #[display("..")]
    Exclusive,
    /// `..=`.
    #[display("..=")]
    Inclusive,
}

/// Transparent or nearly-transparent expression wrapper understood by cheap type normalization.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    wincode::SchemaRead,
    wincode::SchemaWrite,
)]
pub enum ExprWrapperKind {
    /// `(<expr>)`.
    #[display("paren")]
    Paren,
    /// `&<expr>` or `&mut <expr>`.
    #[display("ref")]
    Ref,
    /// `<expr>.await`.
    #[display("await")]
    Await,
    /// `<expr>?`.
    #[display("try")]
    Try,
    /// `return` or `return <expr>`.
    #[display("return")]
    Return,
}

/// One match arm with its pattern scope and lowered arm expression.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct MatchArmData {
    pub pat: Option<PatId>,
    pub scope: ScopeId,
    pub guard: Option<ExprId>,
    pub expr: Option<ExprId>,
}

/// One closure parameter pattern and its lowered bindings.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct ClosureParamData {
    pub source: BodySource,
    pub pat: Option<PatId>,
    pub bindings: Vec<BindingId>,
    pub annotation: Option<TypeRef>,
}

/// A loop label written on loop-like syntax or referenced from a jump expression.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct LabelData {
    pub name: Name,
    pub span: Span,
}

/// Literal category used for display and future cheap inference.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    wincode::SchemaRead,
    wincode::SchemaWrite,
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
                kind,
                label,
                statements,
                tail,
                ..
            } => {
                kind.shrink_to_fit();
                if let Some(label) = label {
                    label.shrink_to_fit();
                }
                statements.shrink_to_fit();
                let _ = tail;
            }
            Self::Path { path } => path.shrink_to_fit(),
            Self::Call { callee, args } => {
                let _ = callee;
                args.shrink_to_fit();
            }
            Self::Tuple { fields } => fields.shrink_to_fit(),
            Self::Array { elements } => elements.shrink_to_fit(),
            Self::Cast { ty, .. } => {
                if let Some(ty) = ty {
                    ty.shrink_to_fit();
                }
            }
            Self::Match { scrutinee, arms } => {
                let _ = scrutinee;
                arms.shrink_to_fit();
            }
            Self::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let _ = condition;
                let _ = then_branch;
                let _ = else_branch;
            }
            Self::Let { bindings, .. } => {
                bindings.shrink_to_fit();
            }
            Self::Closure { params, ret_ty, .. } => {
                params.shrink_to_fit();
                for param in params {
                    param.shrink_to_fit();
                }
                if let Some(ret_ty) = ret_ty {
                    ret_ty.shrink_to_fit();
                }
            }
            Self::For {
                label, bindings, ..
            } => {
                if let Some(label) = label {
                    label.shrink_to_fit();
                }
                bindings.shrink_to_fit();
            }
            Self::Loop { label, .. }
            | Self::While { label, .. }
            | Self::Break { label, .. }
            | Self::Continue { label } => {
                if let Some(label) = label {
                    label.shrink_to_fit();
                }
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
            Self::Record { path, fields, .. } => {
                if let Some(path) = path {
                    path.shrink_to_fit();
                }
                fields.shrink_to_fit();
                for field in fields {
                    field.shrink_to_fit();
                }
            }
            Self::RepeatArray { .. }
            | Self::Index { .. }
            | Self::Range { .. }
            | Self::Unary { .. }
            | Self::Binary { .. }
            | Self::Assign { .. }
            | Self::Wrapper { .. }
            | Self::Literal { .. }
            | Self::Underscore
            | Self::Yield { .. }
            | Self::Yeet { .. }
            | Self::Become { .. } => {}
            Self::Unknown { children } => children.shrink_to_fit(),
        }
    }
}

impl RecordExprField {
    fn shrink_to_fit(&mut self) {
        self.key.shrink_to_fit();
    }
}

impl ClosureParamData {
    fn shrink_to_fit(&mut self) {
        self.bindings.shrink_to_fit();
        if let Some(annotation) = &mut self.annotation {
            annotation.shrink_to_fit();
        }
    }
}

impl LabelData {
    fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
    }
}
