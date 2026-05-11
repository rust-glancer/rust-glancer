//! Cheap expression/type normalization for common body wrappers.
//!
//! This module is intentionally shallow. It keeps everyday editor flows useful through references,
//! parentheses, `.await`, and `?`, but it does not try to implement borrow checking, autoderef, the
//! `Try` trait, or `Future::Output` projection.

use rg_semantic_ir::SemanticIrReadTxn;

use crate::{
    body::BodyData,
    expr::ExprWrapperKind,
    ty::{BodyGenericArg, BodyTy},
};

use super::push_unique;

pub(super) struct BodyTyNormalizer<'db, 'body> {
    semantic_ir: &'db SemanticIrReadTxn<'db>,
    body: &'body BodyData,
}

impl<'db, 'body> BodyTyNormalizer<'db, 'body> {
    pub(super) fn new(semantic_ir: &'db SemanticIrReadTxn<'db>, body: &'body BodyData) -> Self {
        Self { semantic_ir, body }
    }

    pub(super) fn ty_for_wrapper(&self, kind: ExprWrapperKind, inner_ty: BodyTy) -> BodyTy {
        match kind {
            ExprWrapperKind::Paren => inner_ty,
            ExprWrapperKind::Ref => BodyTy::reference(inner_ty),
            // We currently model `async fn foo() -> T` as returning `T` directly. Preserving the
            // inner type through `.await` keeps that useful behavior without pretending to model
            // `Future::Output` for arbitrary future types.
            ExprWrapperKind::Await => inner_ty,
            ExprWrapperKind::Try => self.try_output_ty(&inner_ty),
            // `return expr` evaluates to `!`; the child expression remains separately lowered and
            // queryable, so callers can still ask about `expr` itself.
            ExprWrapperKind::Return => BodyTy::Never,
        }
    }

    fn try_output_ty(&self, ty: &BodyTy) -> BodyTy {
        let mut outputs = Vec::new();

        for nominal in ty.nominal_tys() {
            let Ok(Some(name)) = self.semantic_ir.type_def_name(nominal.def) else {
                continue;
            };
            if matches!(name, "Result" | "Option") {
                if let Some(output) = first_type_arg(&nominal.args) {
                    push_unique(&mut outputs, output);
                }
            }
        }

        for local in ty.local_nominals() {
            let Some(item) = self.body.local_item(local.item.item) else {
                continue;
            };
            if matches!(item.name.as_str(), "Result" | "Option") {
                if let Some(output) = first_type_arg(&local.args) {
                    push_unique(&mut outputs, output);
                }
            }
        }

        one_ty_or_unknown(outputs)
    }
}

fn first_type_arg(args: &[BodyGenericArg]) -> Option<BodyTy> {
    args.iter().find_map(|arg| match arg {
        BodyGenericArg::Type(ty) => Some((**ty).clone()),
        BodyGenericArg::Lifetime(_)
        | BodyGenericArg::Const(_)
        | BodyGenericArg::AssocType { .. }
        | BodyGenericArg::Unsupported(_) => None,
    })
}

fn one_ty_or_unknown(mut tys: Vec<BodyTy>) -> BodyTy {
    if tys.len() == 1 {
        tys.pop().expect("one type should exist")
    } else {
        BodyTy::Unknown
    }
}
