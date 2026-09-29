//! Direct calls selected through a method name or an associated function path.

use anyhow::Context as _;
use rg_body_ir::{BodyView, CallFacts, ExprKind};
use rg_ir_model::{ExprId, identity::ExprRef};

use crate::IndexedViewDb;

/// Connect a selected expression to its call and the body that supplies its resolution facts.
///
/// In `user.name()`, the method name selects the call itself. In `User::name(&user)`, the
/// selected path is a separate expression from the call. Keep that distinction here so callers
/// can read the selected function, generic arguments, and receiver without rediscovering the
/// expression layout. A call can still have declaration or receiver facts without `CallFacts`.
pub struct BodyCallView<'a> {
    pub(crate) expr: ExprRef,
    pub(crate) body: BodyView<'a>,
    call: ExprId,
    pub(crate) receiver: Option<ExprId>,
}

impl<'a> BodyCallView<'a> {
    pub fn for_expr(db: &'a IndexedViewDb<'_>, expr: ExprRef) -> anyhow::Result<Option<Self>> {
        let Some(body) = db.body_ir.body(expr.body_ir()).context("read call body")? else {
            return Ok(None);
        };
        let Some(data) = body.expr(expr.expr_id()) else {
            return Ok(None);
        };

        // An associated path gets its final substitutions from the enclosing call: the
        // arguments in `Convert::convert(&source)` can determine Self and the trait arguments.
        // Only follow direct callees. A path stored in a local variable has no such call context.
        let (call, receiver) = match &data.kind {
            ExprKind::MethodCall { receiver, .. } => (expr.expr_id(), *receiver),
            ExprKind::Path { path } if path.split_associated_item_prefix_name().is_some() => {
                let Some(call) = body.exprs().iter().position(|candidate| {
                    matches!(candidate.kind, ExprKind::Call { callee: Some(callee), .. }
                        if callee == expr.expr_id())
                }) else {
                    return Ok(None);
                };
                (ExprId(call), None)
            }
            _ => return Ok(None),
        };

        Ok(Some(Self {
            expr,
            body,
            call,
            receiver,
        }))
    }

    pub fn facts(&self) -> Option<&'a CallFacts> {
        self.body.call_facts(self.call)
    }
}
