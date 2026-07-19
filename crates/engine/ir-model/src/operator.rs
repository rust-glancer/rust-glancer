use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Unary operator whose primitive typing is independent of one lowered body.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    SchemaRead,
    SchemaWrite,
    MemorySize,
    Shrink,
)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum ExprUnaryOp {
    #[display("*")]
    Deref,
    #[display("!")]
    Not,
    #[display("-")]
    Neg,
}

/// Binary operator whose primitive typing is independent of one lowered body.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    SchemaRead,
    SchemaWrite,
    MemorySize,
    Shrink,
)]
#[memsize(leaf)]
#[shrink(leaf)]
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
