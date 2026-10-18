use rg_ir_model::{ExprBinaryOp, ExprUnaryOp, PrimitiveTy};
use rg_item_tree::ConstExprData;

/// Read-only access to expressions in the caller's storage.
///
/// Body IR and temporary type-level syntax supply the same small set of operations. The evaluator
/// owns the recursive walk; adapters only describe individual nodes, without copying a body.
/// A body source uses expression ids, while temporary syntax uses references to its parsed nodes.
pub trait ConstExprSource {
    type Expr: Copy;

    /// `None` means the node cannot be evaluated, even if some of its children are supported.
    fn expr(&self, expr: Self::Expr) -> Option<ConstExprView<Self::Expr>>;
}

/// The expression forms understood by const evaluation, borrowing child identities from a source.
#[derive(Debug, Clone, Copy)]
pub enum ConstExprView<E> {
    Integer {
        value: u128,
        suffix: Option<PrimitiveTy>,
    },
    Unary {
        op: ExprUnaryOp,
        expr: E,
    },
    Binary {
        op: ExprBinaryOp,
        lhs: E,
        rhs: E,
    },
    /// Parentheses or a statement-free block whose value comes directly from its child.
    Transparent(E),
}

impl<'a> ConstExprSource for &'a ConstExprData {
    type Expr = &'a ConstExprData;

    fn expr(&self, expr: Self::Expr) -> Option<ConstExprView<Self::Expr>> {
        Some(match expr {
            ConstExprData::Integer { value, suffix } => ConstExprView::Integer {
                value: *value,
                suffix: *suffix,
            },
            ConstExprData::Unary { op, expr } => ConstExprView::Unary { op: *op, expr },
            ConstExprData::Binary { op, lhs, rhs } => ConstExprView::Binary { op: *op, lhs, rhs },
        })
    }
}
