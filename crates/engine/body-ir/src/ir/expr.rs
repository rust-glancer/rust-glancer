use std::fmt;

use wincode::{SchemaRead, SchemaWrite};

use rg_ir_model::{
    BindingId, BodySource, ClosureCapture, ClosureKind, ExprAssignOp, ExprBinaryOp, ExprId,
    ExprRangeKind, ExprUnaryOp, LabelData, PatId, ScopeId, StmtId,
};
use rg_item_tree::{FieldKey, GenericArg, TypeRef};
use rg_memsize::MemorySize;
use rg_parse::Span;
use rg_text::Name;
use rg_ty::{PrimitiveTy, RefMutability, Ty};

use super::{RecordFieldSyntax, path::BodyPath};

/// One lowered expression.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct ExprData {
    pub source: BodySource,
    pub scope: ScopeId,
    /// Number of body-wide bindings that were visible at this expression's source location.
    ///
    /// Scope data is frozen after lowering, so the resolver needs this boundary to avoid letting a
    /// later `let x` shadow an earlier use of `x`.
    pub visible_bindings: usize,
    pub kind: ExprKind,
}

impl ExprData {
    pub(crate) fn shrink_to_fit(&mut self) {
        self.kind.shrink_to_fit();
    }
}

/// One field written inside a record expression.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct RecordExprField {
    pub key: FieldKey,
    pub key_span: Span,
    pub source_span: Span,
    pub syntax: RecordFieldSyntax,
    pub value: Option<ExprId>,
}

/// `..` or `..base` written after record fields.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct RecordExprSpread {
    pub source_span: Span,
    pub expr: Option<ExprId>,
}

/// Block-level execution modifier written before the statement list.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub enum ExprBlockKind {
    /// `{ ... }`.
    Plain,
    /// `unsafe { ... }`.
    Unsafe,
    /// `const { ... }`.
    Const,
    /// `async { ... }` or `async move { ... }`.
    Async {
        #[memsize(skip)]
        move_capture: bool,
    },
    /// `try { ... }` or `try bikeshed Type { ... }`.
    Try {
        #[memsize(skip)]
        bikeshed: bool,
        result_ty: Option<TypeRef>,
    },
    /// `gen { ... }` or `gen move { ... }`.
    Gen {
        #[memsize(skip)]
        move_capture: bool,
    },
    /// `async gen { ... }` or `async gen move { ... }`.
    AsyncGen {
        #[memsize(skip)]
        move_capture: bool,
    },
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
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
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
        len_text: Option<String>,
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
        /// Explicit method generic arguments written after the method name, such as
        /// `receiver.get::<User>()`.
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<GenericArg>>")]
        generic_args: Vec<GenericArg>,
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

/// Transparent or nearly-transparent expression wrapper understood by cheap type normalization.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, derive_more::Display, SchemaRead, SchemaWrite, MemorySize,
)]
#[memsize(leaf)]
pub enum ExprWrapperKind {
    /// `(<expr>)`.
    #[display("paren")]
    Paren,
    /// `&<expr>` or `&mut <expr>`.
    #[display("ref")]
    Ref { mutability: RefMutability },
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
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct MatchArmData {
    pub pat: Option<PatId>,
    pub scope: ScopeId,
    pub guard: Option<ExprId>,
    pub expr: Option<ExprId>,
}

/// One closure parameter pattern and its lowered bindings.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct ClosureParamData {
    pub source: BodySource,
    pub pat: Option<PatId>,
    pub bindings: Vec<BindingId>,
    pub annotation: Option<TypeRef>,
}

/// Literal category plus the primitive type implied by suffix/default heuristics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
#[memsize(leaf)]
pub enum LiteralKind {
    Bool,
    Char,
    Float { primitive_ty: Option<PrimitiveTy> },
    Int { primitive_ty: Option<PrimitiveTy> },
    String,
    Unknown,
}

impl LiteralKind {
    pub fn ty(self) -> Ty {
        match self {
            Self::Bool => Ty::Primitive(PrimitiveTy::Bool),
            Self::Char => Ty::Primitive(PrimitiveTy::Char),
            Self::Float { primitive_ty } | Self::Int { primitive_ty } => {
                primitive_ty.map(Ty::Primitive).unwrap_or(Ty::Unknown)
            }
            Self::String => Ty::reference(RefMutability::Shared, Ty::Primitive(PrimitiveTy::Str)),
            Self::Unknown => Ty::Unknown,
        }
    }
}

impl fmt::Display for LiteralKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool => write!(f, "bool"),
            Self::Char => write!(f, "char"),
            Self::Float { .. } => write!(f, "float"),
            Self::Int { .. } => write!(f, "int"),
            Self::String => write!(f, "string"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
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
                generic_args,
                args,
                ..
            } => {
                let _ = receiver;
                method_name.shrink_to_fit();
                generic_args.shrink_to_fit();
                for arg in generic_args {
                    arg.shrink_to_fit();
                }
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
            Self::RepeatArray { len_text, .. } => {
                if let Some(len_text) = len_text {
                    len_text.shrink_to_fit();
                }
            }
            Self::Index { .. }
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
