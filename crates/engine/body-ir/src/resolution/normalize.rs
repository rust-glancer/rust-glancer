//! Cheap expression/type normalization for common body wrappers.
//!
//! This module is intentionally shallow. It keeps everyday editor flows useful through references,
//! parentheses, `.await`, and `?`, but it does not try to implement borrow checking, autoderef, the
//! `Try` trait, or `Future::Output` projection.

use rg_semantic_ir::{ItemStoreQuery, SemanticIrReadTxn};
use rg_ty::{GenericArg, Ty};

use crate::{ir::body::BodyData, ir::expr::ExprWrapperKind};

use super::{item_query::BodyItemStoreSource, push_unique};

pub(super) struct TyNormalizer<'db, 'body> {
    semantic_ir: &'db SemanticIrReadTxn<'db>,
    body_ref: rg_ir_model::BodyRef,
    body: &'body BodyData,
}

impl<'db, 'body> TyNormalizer<'db, 'body> {
    pub(super) fn new(
        semantic_ir: &'db SemanticIrReadTxn<'db>,
        body_ref: rg_ir_model::BodyRef,
        body: &'body BodyData,
    ) -> Self {
        Self {
            semantic_ir,
            body_ref,
            body,
        }
    }

    pub(super) fn ty_for_wrapper(&self, kind: ExprWrapperKind, inner_ty: Ty) -> Ty {
        match kind {
            ExprWrapperKind::Paren => inner_ty,
            ExprWrapperKind::Ref { mutability } => Ty::reference(mutability, inner_ty),
            // We currently model `async fn foo() -> T` as returning `T` directly. Preserving the
            // inner type through `.await` keeps that useful behavior without pretending to model
            // `Future::Output` for arbitrary future types.
            ExprWrapperKind::Await => inner_ty,
            ExprWrapperKind::Try => self.try_output_ty(&inner_ty),
            // `return expr` evaluates to `!`; the child expression remains separately lowered and
            // queryable, so callers can still ask about `expr` itself.
            ExprWrapperKind::Return => Ty::Never,
        }
    }

    fn try_output_ty(&self, ty: &Ty) -> Ty {
        let mut outputs = Vec::new();
        let item_query = ItemStoreQuery::new(BodyItemStoreSource::new(
            self.semantic_ir,
            self.body_ref,
            self.body,
        ));

        for nominal in ty.as_nominals() {
            let Ok(Some(name)) = item_query.type_def_name(nominal.def) else {
                continue;
            };
            if matches!(name, "Result" | "Option") {
                if let Some(output) = first_type_arg(&nominal.args) {
                    push_unique(&mut outputs, output);
                }
            }
        }

        Ty::one_or_unknown(outputs)
    }
}

fn first_type_arg(args: &[GenericArg]) -> Option<Ty> {
    args.iter().find_map(|arg| arg.as_ty().cloned())
}
