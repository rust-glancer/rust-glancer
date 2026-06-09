//! Main body-resolution pass.
//!
//! This module walks lowered bodies and fills resolution/type slots on bindings and expressions.
//! Specialized helpers live in sibling modules so this file can read like the pass itself.

use rg_ir_model::{BindingId, BodyRef, ExprId};
use rg_ir_storage::{DefMapSource, ItemLookupIndex, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_ty::Ty;

use crate::{
    ir::body::ResolvedBodyData,
    ir::{BindingKind, BodySelfParamKind},
};

use crate::resolution::{BodyResolutionContext, BodyResolutionProviders, TypeRefUseSite};

use super::{
    expr::ExprResolutionPass, pattern_binding::PatternBindingMaterializationPass,
    pattern_type::PatternTypePropagationPass,
};

/// Shared state for the body-resolution fixed-point pass.
///
/// Sibling pass modules keep their logic in separate files while operating on the same body
/// facts, so the fields are scoped to `resolution` rather than hidden inside this file.
pub(crate) struct BodyResolutionPass<'query, 'body, D, I> {
    pub(super) providers: BodyResolutionProviders<'query, D, I>,
    pub(super) body: &'body mut ResolvedBodyData,
}

impl<'query, 'body, D, I> BodyResolutionPass<'query, 'body, D, I>
where
    for<'source> &'source D: DefMapSource<Error = PackageStoreError>,
    for<'source> &'source I: ItemStoreSource<'source, Error = PackageStoreError>,
{
    pub(crate) fn new(
        def_maps: &'query D,
        item_stores: &'query I,
        semantic_index: &'query ItemLookupIndex,
        body_ref: BodyRef,
        body: &'body mut ResolvedBodyData,
    ) -> Self {
        Self {
            providers: BodyResolutionProviders::new(
                def_maps,
                item_stores,
                semantic_index,
                body_ref,
            ),
            body,
        }
    }

    pub(super) fn context<'source>(
        &'source self,
    ) -> BodyResolutionContext<'source, &'source D, &'source I> {
        self.providers.context(self.body)
    }

    pub(crate) fn resolve(&mut self) -> Result<(), PackageStoreError> {
        self.materialize_pattern_bindings()?;
        self.resolve_bindings()?;

        // Pattern propagation can unlock later expression types, and those expressions can then
        // unlock more patterns. Every successful pass should discover at least one new binding or
        // expression fact, so a body-sized cap is enough to avoid a hidden magic constant.
        let max_passes = self.body.exprs().len() + self.body.bindings().len() + 1;
        for _ in 0..max_passes {
            let mut changed = false;
            let expr_count = self.body.exprs().len();
            {
                let mut expr_pass = ExprResolutionPass::new(self);
                for expr_idx in 0..expr_count {
                    changed |= expr_pass.resolve_expr(ExprId(expr_idx))?;
                }
            }
            let binding_updates = PatternTypePropagationPass::new(self.context()).propagate()?;
            changed |= self.apply_binding_type_updates(binding_updates);

            if !changed {
                break;
            }
        }

        Ok(())
    }

    fn materialize_pattern_bindings(&mut self) -> Result<(), PackageStoreError> {
        PatternBindingMaterializationPass::new(self.providers, self.body).materialize()
    }

    fn resolve_bindings(&mut self) -> Result<(), PackageStoreError> {
        for binding_idx in 0..self.body.bindings().len() {
            let binding = BindingId(binding_idx);
            let ty = self.binding_ty(binding)?;
            self.body.set_binding_ty(binding, ty);
        }
        Ok(())
    }

    fn apply_binding_type_updates(&mut self, updates: Vec<(BindingId, Ty)>) -> bool {
        let mut changed = false;
        for (binding, ty) in updates {
            if matches!(ty, Ty::Unknown) {
                continue;
            }

            if self.body.binding(binding).is_none() {
                continue;
            };
            if !matches!(self.body.binding_ty_unchecked(binding), Ty::Unknown) {
                continue;
            }

            self.body.set_binding_ty(binding, ty);
            changed = true;
        }

        changed
    }

    fn binding_ty(&self, binding: BindingId) -> Result<Ty, PackageStoreError> {
        let binding_data = self.body.binding_unchecked(binding);
        if let Some(annotation) = &binding_data.annotation {
            return self
                .context()
                .type_path_query()
                .type_ref(TypeRefUseSite::Scope(binding_data.scope))
                .resolve(annotation);
        }

        if let BindingKind::SelfParam(kind) = binding_data.kind
            && binding_data.name.as_deref() == Some("self")
            && let Some(function) = self.body.function_owner()
        {
            let self_tys = self
                .context()
                .type_path_query()
                .self_nominal_tys_for_function(function)?;
            let ty = Ty::self_ty(self_tys);
            return Ok(match kind {
                BodySelfParamKind::Value => ty,
                BodySelfParamKind::Reference { mutability } => Ty::reference(mutability, ty),
                BodySelfParamKind::Explicit => Ty::Unknown,
            });
        }

        Ok(Ty::Unknown)
    }
}
