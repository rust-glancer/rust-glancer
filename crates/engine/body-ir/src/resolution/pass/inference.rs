//! Inference-facing helpers for the body-resolution pass.
//!
//! Body resolution still publishes ordinary `Ty` facts while it runs. This module collects direct
//! constraints over the parallel inference view and writes the finalized inference facts back into
//! Body IR.

use rg_ir_model::{BindingId, ExprId, StmtId};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_ty::Ty;

use crate::{ir::StmtKind, resolution::TypeRefUseSite};

use super::body::BodyResolutionPass;

/// Collects body-local inference constraints that need the fixed-point facts to be available.
///
/// The pass runs after normal body resolution has stabilized: it adds direct constraints such as
/// annotated `let` initializers, then collapses inference variables back into ordinary `Ty` facts.
pub(super) struct InferenceResolutionPass<'pass, 'query, 'body, D, I> {
    pass: &'pass mut BodyResolutionPass<'query, 'body, D, I>,
}

impl<'pass, 'query, 'body, D, I> InferenceResolutionPass<'pass, 'query, 'body, D, I> {
    pub(super) fn new(pass: &'pass mut BodyResolutionPass<'query, 'body, D, I>) -> Self {
        Self { pass }
    }
}

impl<'pass, 'query, 'body, D, I> InferenceResolutionPass<'pass, 'query, 'body, D, I>
where
    for<'source> &'source D: DefMapSource<Error = PackageStoreError>,
    for<'source> &'source I: ItemStoreSource<'source, Error = PackageStoreError>,
{
    pub(super) fn run(mut self) -> Result<(), PackageStoreError> {
        self.constrain_let_annotation_initializers()?;
        self.finalize_facts();
        Ok(())
    }

    /// Constrains initializer expression slots from explicit statement annotations.
    ///
    /// This intentionally only links the annotation to the initializer root expression. Nested
    /// expected-type propagation belongs in expression kind-specific rules.
    fn constrain_let_annotation_initializers(&mut self) -> Result<(), PackageStoreError> {
        for statement_idx in 0..self.pass.body.statements().len() {
            let StmtKind::Let {
                scope,
                annotation: Some(annotation),
                initializer: Some(initializer),
                ..
            } = self
                .pass
                .body
                .statement_unchecked(StmtId(statement_idx))
                .kind
                .clone()
            else {
                continue;
            };

            let expected_ty = self
                .pass
                .context()
                .type_path_query()
                .type_ref(TypeRefUseSite::Scope(scope))
                .resolve(&annotation)?;
            if matches!(expected_ty, Ty::Unknown) {
                continue;
            }

            // A statement annotation is direct evidence only for the initializer expression
            // itself. Nested expected-type propagation belongs in expression-specific rules.
            if let Some(inference) = &mut self.pass.inference {
                inference.constrain_expr_ty(initializer, &expected_ty);
            }
        }

        Ok(())
    }

    /// Writes finalized inference facts back into the body.
    ///
    /// Downstream queries only read `Ty`, so unresolved variables are defaulted or erased here
    /// before the resolved body leaves this pass.
    fn finalize_facts(&mut self) {
        let Some(inference) = self.pass.inference.take() else {
            return;
        };

        // Persist the inference view back into Body IR so downstream queries see ordinary `Ty`
        // facts. Unsolved numeric variables become defaults; conflicts become `<unknown>`.
        for expr_idx in 0..self.pass.body.exprs().len() {
            let expr = ExprId(expr_idx);
            self.pass
                .body
                .set_expr_ty(expr, inference.finalize_expr_ty(expr));
        }
        for binding_idx in 0..self.pass.body.bindings().len() {
            let binding = BindingId(binding_idx);
            self.pass
                .body
                .set_binding_ty(binding, inference.finalize_binding_ty(binding));
        }
    }
}
