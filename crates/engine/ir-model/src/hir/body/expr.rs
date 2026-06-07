use wincode::{SchemaRead, SchemaWrite};

use rg_memsize::MemorySize;

/// Closure capture mode written before the closure parameter list.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, derive_more::Display, SchemaRead, SchemaWrite, MemorySize,
)]
#[memsize(leaf)]
pub enum ClosureCapture {
    #[display("inferred")]
    Inferred,
    #[display("move")]
    Move,
}

/// Closure-level execution modifier that affects how its body is interpreted.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, derive_more::Display, SchemaRead, SchemaWrite, MemorySize,
)]
#[memsize(leaf)]
pub enum ClosureKind {
    #[display("normal")]
    Normal,
    #[display("async")]
    Async,
}

/// Unary operator written before an expression.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, derive_more::Display, SchemaRead, SchemaWrite, MemorySize,
)]
#[memsize(leaf)]
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
    Debug, Clone, Copy, PartialEq, Eq, derive_more::Display, SchemaRead, SchemaWrite, MemorySize,
)]
#[memsize(leaf)]
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

impl ExprBinaryOp {
    pub fn is_logical(self) -> bool {
        matches!(self, Self::LogicOr | Self::LogicAnd)
    }

    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            Self::Eq | Self::NotEq | Self::Less | Self::LessEq | Self::Greater | Self::GreaterEq
        )
    }
}

/// Assignment operator written between a target expression and a value expression.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, derive_more::Display, SchemaRead, SchemaWrite, MemorySize,
)]
#[memsize(leaf)]
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
    Debug, Clone, Copy, PartialEq, Eq, derive_more::Display, SchemaRead, SchemaWrite, MemorySize,
)]
#[memsize(leaf)]
pub enum ExprRangeKind {
    /// `..`.
    #[display("..")]
    Exclusive,
    /// `..=`.
    #[display("..=")]
    Inclusive,
}
