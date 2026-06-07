//! Cheap expression/type normalization for common body wrappers.
//!
//! This module is intentionally shallow. It keeps everyday editor flows useful through references,
//! parentheses, `.await`, and `?`, but it does not try to implement borrow checking, autoderef, the
//! `Try` trait, or `Future::Output` projection.

use rg_ir_storage::{DefMapSource, ItemStoreQuery, ItemStoreSource};
use rg_ty::{GenericArg, Ty};

use crate::ir::ExprWrapperKind;

use rg_package_store::PackageStoreError;

use crate::resolution::source::BodyQuerySource;

use super::push_unique;

pub(crate) struct TyNormalizer<'a, D, I> {
    source: BodyQuerySource<'a, D, I>,
}

impl<'a, D, I> TyNormalizer<'a, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'a, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(source: BodyQuerySource<'a, D, I>) -> Self {
        Self { source }
    }

    pub(crate) fn ty_for_wrapper(&self, kind: ExprWrapperKind, inner_ty: Ty) -> Ty {
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
        let item_query = ItemStoreQuery::new(self.source);

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
