//! Const evaluation over ordinary lowered bodies.
//!
//! Saved declarations use a temporary initializer worklist even if function bodies are skipped.
//! Local and editor declarations reuse bodies already being built for their lexical context.
//! Both paths resolve annotation types through the body context and use the shared `rg_ty`
//! evaluator. Concrete results go to the declaration's ItemStore; the expression inputs can be dropped.

use rg_def_map::DefMapSource;
use rg_ir_model::{BodyRef, ConstRef, CrateRef, EvaluatedConstValue, ExprId};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemLookupQuery, ItemLookupQueryCache, ItemStoreQuery, ItemStoreSource};
use rg_ty::{
    Ty,
    const_eval::{ConstEvaluationQuery, ConstExprSource, ConstExprView},
    trait_selection::TraitSelectionQuery,
};

use super::{
    BodyIrBuilder,
    lower::{self, LoweredBodyData},
    materialization::BodyIrMaterialization,
};
use crate::{
    BodyOwner, ExprBlockKind, ExprKind, ExprWrapperKind, LiteralKind,
    resolution::BodyResolutionContext,
};

impl BodyIrBuilder<'_, '_> {
    /// Evaluate saved const declarations without requiring function-body indexing.
    ///
    /// Initializers use ordinary body lowering and semantic type queries. Only their final values
    /// escape; the coordinator installs them in ItemStore before publishing the saved generation.
    /// This visits consts in the selected packages regardless of body policy or file selection.
    pub fn evaluate_consts(self) -> anyhow::Result<Vec<(ConstRef, EvaluatedConstValue)>> {
        let packages = Self::normalized_package_slots(self.packages);
        let semantic_ir = self
            .semantic_ir
            .read_txn_for_subset(self.semantic_ir_loader, self.subset);
        let def_map = self
            .def_map
            .read_txn_for_subset(self.def_map_loader, self.subset);
        // Keep initializer inputs transient even when function-body analysis is deferred.
        // TODO: Reuse this worklist when ordinary body indexing immediately follows.
        let lowered = lower::build_selected_packages(
            self.parse,
            &def_map,
            &semantic_ir,
            BodyIrMaterialization::ConstInitializers,
            &packages,
            self.interners,
            self.worker_limit,
            None,
            &self.cancellation,
        )?;
        // Each initializer needs its own crate's names and impls. Reuse dependency lookup work
        // across those contexts while resolving their declaration types.
        let cache = ItemLookupQueryCache::new();
        let mut values = Vec::new();
        for (package, crates) in lowered {
            for (crate_id, bodies) in crates {
                if bodies.bodies().is_empty() {
                    continue;
                }
                let crate_ref = CrateRef { package, crate_id };
                let lookup = ItemLookupQuery::build_with_cache(
                    &ItemStoreQuery::new(&semantic_ir),
                    crate_ref,
                    &cache,
                    &self.cancellation,
                )?;
                for (id, body) in bodies.bodies().iter_with_ids() {
                    rg_std::check_cancel!(self.cancellation, "saved const values");
                    let context = BodyResolutionContext::new(
                        &def_map,
                        &semantic_ir,
                        BodyRef {
                            crate_ref,
                            body: id,
                        },
                        body.body(),
                        &lookup,
                        self.cancellation.clone(),
                    );
                    if let Some(value) = context.evaluate_const(body)? {
                        values.push(value);
                    }
                }
            }
        }
        Ok(values)
    }
}

impl ConstExprSource for LoweredBodyData {
    type Expr = ExprId;

    fn expr(&self, expr: ExprId) -> Option<ConstExprView<ExprId>> {
        Some(match &self.body().expr(expr)?.kind {
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
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'a, Error = PackageStoreError> + Copy,
{
    /// The declaration's annotation is interpreted by the same lowerer as body type inference,
    /// including aliases, generic arguments and `Self`. Evaluation only consumes its result.
    /// For `type Size = usize; const N: Size = 2 + 2;`, the evaluator receives `usize`.
    pub(crate) fn evaluate_const(
        &self,
        lowered: &LoweredBodyData,
    ) -> anyhow::Result<Option<(ConstRef, EvaluatedConstValue)>> {
        let BodyOwner::Const(konst) = lowered.body().owner() else {
            return Ok(None);
        };
        let Some(ty) = self.signatures().const_ty(konst)? else {
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
            .target_pointer_width(konst.origin.origin_crate())?;
        Ok(ConstEvaluationQuery::new(lowered, pointer_width)
            .evaluate_expr(lowered.body().root_expr(), ty)
            .map(|value| (konst, value)))
    }
}
