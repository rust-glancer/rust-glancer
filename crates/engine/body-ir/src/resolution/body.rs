//! Main body-resolution pass.
//!
//! This module walks lowered bodies and fills resolution/type slots on bindings and expressions.
//! Specialized helpers live in sibling modules so this file can read like the pass itself.

use rg_ir_model::{BindingId, BodyRef, ExprId};
use rg_ir_storage::{
    DefMapSource, ItemLookupIndex, ItemStoreQuery, ItemStoreSource, TargetItemQuery,
};
use rg_package_store::PackageStoreError;
use rg_ty::{Autoderef, ImplMatcher, ItemPathQuery, Ty};

use crate::{
    ir::body::ResolvedBodyData,
    ir::{BindingKind, BodySelfParamKind},
};

use super::{
    BodyQuerySource, BodyReceiverFunctionQuery, TypeRefUseSite, expr::ExprResolver,
    pat::PatternTypePropagator, pat_binding::PatternBindingMaterializer,
    type_path::BodyTypePathResolver,
};

/// Shared state for the body-resolution fixed-point pass.
///
/// Sibling resolver modules keep their logic in separate files while operating on the same body
/// facts, so the fields are scoped to `resolution` rather than hidden inside this file.
pub(crate) struct BodyResolver<'query, 'body, D, I> {
    pub(super) def_maps: &'query D,
    pub(super) item_stores: &'query I,
    pub(super) semantic_index: &'query ItemLookupIndex,
    pub(super) body_ref: BodyRef,
    pub(super) body: &'body mut ResolvedBodyData,
}

impl<'query, 'body, D, I> BodyResolver<'query, 'body, D, I>
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
            def_maps,
            item_stores,
            semantic_index,
            body_ref,
            body,
        }
    }

    pub(super) fn type_path_resolver<'source>(
        &'source self,
    ) -> BodyTypePathResolver<'source, &'source D, &'source I> {
        BodyTypePathResolver::new(self.query_source())
    }

    pub(super) fn query_source<'source>(
        &'source self,
    ) -> BodyQuerySource<'source, &'source D, &'source I> {
        BodyQuerySource::new(self.def_maps, self.item_stores, self.body_ref, self.body)
    }

    pub(super) fn autoderef(
        &self,
    ) -> Autoderef<'_, BodyQuerySource<'_, &D, &I>, BodyQuerySource<'_, &D, &I>> {
        let source = self.query_source();
        let item_paths = ItemPathQuery::new(source, source);
        let target_items = TargetItemQuery::new(source, source, self.body_ref.target);
        Autoderef::with_index(item_paths, target_items, self.semantic_index)
    }

    pub(super) fn impl_matcher(
        &self,
    ) -> ImplMatcher<'_, BodyQuerySource<'_, &D, &I>, BodyQuerySource<'_, &D, &I>> {
        let source = self.query_source();
        let item_paths = ItemPathQuery::new(source, source);
        let target_items = TargetItemQuery::new(source, source, self.body_ref.target);
        ImplMatcher::new(item_paths, target_items)
    }

    pub(super) fn item_query(&self) -> ItemStoreQuery<'_, BodyQuerySource<'_, &D, &I>> {
        ItemStoreQuery::new(self.query_source())
    }

    pub(super) fn receiver_functions<'source>(
        &'source self,
    ) -> BodyReceiverFunctionQuery<'source, &'source D, &'source I> {
        BodyReceiverFunctionQuery::new(self.query_source(), Some(self.semantic_index))
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
                let mut expr_resolver = ExprResolver::new(self);
                for expr_idx in 0..expr_count {
                    changed |= expr_resolver.resolve_expr(ExprId(expr_idx))?;
                }
            }
            let binding_updates =
                PatternTypePropagator::new(self.query_source(), self.semantic_index).propagate()?;
            changed |= self.apply_binding_type_updates(binding_updates);

            if !changed {
                break;
            }
        }

        Ok(())
    }

    fn materialize_pattern_bindings(&mut self) -> Result<(), PackageStoreError> {
        PatternBindingMaterializer::new(
            self.def_maps,
            self.item_stores,
            self.semantic_index,
            self.body_ref,
            self.body,
        )
        .materialize()
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
                .type_path_resolver()
                .type_ref(TypeRefUseSite::Scope(binding_data.scope))
                .resolve(annotation);
        }

        if let BindingKind::SelfParam(kind) = binding_data.kind
            && binding_data.name.as_deref() == Some("self")
            && let Some(function) = self.body.function_owner()
        {
            let self_tys = self
                .type_path_resolver()
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
