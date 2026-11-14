//! Evaluate the supported initializer expressions when producing body facts.

use rg_def_map::DefMapSource;
use rg_ir_model::{ConstOrStaticRef, EvaluatedConstValue, ExprId};
use rg_semantic_ir::SemanticItemSource;
use rg_ty::{
    Ty,
    const_eval::{ConstEvaluationQuery, ConstExprSource, ConstExprView},
    trait_selection::TraitSelectionQuery,
};

use crate::{
    BodyData, BodyOwner, ExprBlockKind, ExprKind, ExprWrapperKind, LiteralKind, profile::metric,
    resolution::BodyResolutionContext,
};

impl ConstExprSource for BodyData {
    type Expr = ExprId;

    fn expr(&self, expr: ExprId) -> Option<ConstExprView<ExprId>> {
        Some(match &BodyData::expr(self, expr)?.kind {
            ExprKind::Literal {
                kind: LiteralKind::Int { primitive_ty },
            } => ConstExprView::Integer {
                value: self.integer_literal(expr)?,
                suffix: *primitive_ty,
            },
            ExprKind::Unary { op, expr } => ConstExprView::Unary {
                op: (*op)?,
                expr: (*expr)?,
            },
            ExprKind::Binary { op, lhs, rhs } => ConstExprView::Binary {
                op: (*op)?,
                lhs: (*lhs)?,
                rhs: (*rhs)?,
            },
            ExprKind::Wrapper {
                kind: ExprWrapperKind::Paren,
                inner,
            } => ConstExprView::Transparent((*inner)?),
            // `{ 2 + 2 }` forwards its tail value. `{ work(); 4 }` needs statement evaluation;
            // taking only its tail would silently skip the preceding work.
            ExprKind::Block {
                kind: ExprBlockKind::Plain,
                statements,
                tail,
                ..
            } if statements.is_empty() => ConstExprView::Transparent((*tail)?),
            // TODO: Resolve const paths and interpret further expression forms through the same
            // body context. Unsupported nodes must leave the entire initializer unknown.
            _ => return None,
        })
    }
}

impl<'a, D, I> BodyResolutionContext<'a, D, I>
where
    D: DefMapSource + Copy,
    I: SemanticItemSource<'a> + Copy,
{
    /// The declaration's annotation is interpreted by the same lowerer as body type inference,
    /// including aliases, generic arguments and `Self`. Evaluation only consumes its result.
    /// For `type Size = usize; const N: Size = 2 + 2;`, the evaluator receives `usize`.
    pub(crate) fn evaluate_initializer(&self) -> anyhow::Result<Option<EvaluatedConstValue>> {
        let item = match self.body().owner() {
            BodyOwner::Const(item) => ConstOrStaticRef::Const(item),
            BodyOwner::Static(item) => ConstOrStaticRef::Static(item),
            BodyOwner::Function(_) => return Ok(None),
        };
        rg_std::check_cancel!(self, "initializer evaluation");
        metric::CONST_INITIALIZERS.inc();
        let _timer = metric::CONST_EVALUATION.start_timer();
        let ty = match item {
            ConstOrStaticRef::Const(item) => self.signatures().const_ty(item)?,
            ConstOrStaticRef::Static(item) => self.signatures().static_ty(item)?,
        };
        let Some(ty) = ty else {
            return Ok(None);
        };
        // An annotation such as `<Buffer as Limits>::Size` can still be a projection. Normalize
        // it before deciding which primitive arithmetic rules apply.
        let Ty::Primitive(ty) =
            TraitSelectionQuery::with_resolver(self.ty_context(), self).normalize_ty(&ty)?
        else {
            return Ok(None);
        };
        let pointer_width = self
            .def_map_source()
            .target_pointer_width(item.origin().origin_crate())?;
        let value = ConstEvaluationQuery::new(self.body(), pointer_width)
            .evaluate_expr(self.body().root_expr(), ty);
        if value.is_some() {
            metric::CONST_KNOWN_VALUES.inc();
        }
        Ok(value)
    }
}
