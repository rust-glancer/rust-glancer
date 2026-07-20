use rg_ir_model::{
    BindingId, BodyRef, ExprId, FunctionRef, ModuleRef, PatId, ScopeId, StmtId,
    identity::DeclarationRef,
};
use rg_item_tree::{ItemNode, ItemTreeId};

use super::{
    body::{
        BindingData, BodyData, BodyMacroCallData, BodyOwner, BodySource, BodySourceItems, ExprData,
        FunctionParamData, PatData, ScopeData, StmtData,
    },
    resolved::{BindingFacts, BodyFacts, BodyResolution, CallFacts, ExprFacts},
};

/// Source of semantic facts exposed to crate-private body queries.
///
/// Finalized queries borrow the persisted sidecar. During inference, queries borrow one coherent
/// snapshot of the live type slots together with the resolutions accumulated by the parent pass.
/// Keeping the choice explicit prevents a query from accidentally mixing finalized and live
/// values for the same expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyQueryFacts<'a> {
    Final(&'a BodyFacts),
    Inference {
        resolutions: &'a BodyFacts,
        expr_tys: &'a [rg_ty::Ty],
        binding_tys: &'a [rg_ty::Ty],
    },
}

/// Finalized read view over one structural body and its persisted semantic sidecar.
///
/// `BodyData` and `BodyFacts` have separate owners so structural IR can be built and frozen before
/// resolution starts. Readers normally need both: `expr(id)` reads the lowered node, while
/// `expr_ty(id)` and `expr_declarations(...)` read conclusions for that same id. This view joins
/// the two without creating a third owning representation.
///
/// Construction expects the dense sidecars to mirror the body's arenas and checks that invariant
/// in debug builds. A `BodyView` is finalized-only; inference-time queries use the narrower
/// crate-private `BodyQueryView` instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyView<'a> {
    body: &'a BodyData,
    facts: &'a BodyFacts,
}

/// Narrow body projection available to semantic queries during indexing.
///
/// Unlike the public finalized view, this type exposes no aggregate fact sidecars or persisted call
/// selections. An inference snapshot therefore cannot accidentally reach an API that only makes
/// sense after [`BodyFacts`] has been finalized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BodyQueryView<'a> {
    body: &'a BodyData,
    facts: BodyQueryFacts<'a>,
}

impl<'a> BodyView<'a> {
    pub(crate) fn new(body: &'a BodyData, facts: &'a BodyFacts) -> Self {
        debug_assert!(
            facts.is_aligned_with(body),
            "body facts should mirror body binding and expression ids",
        );
        Self { body, facts }
    }

    pub(crate) fn query_view(self) -> BodyQueryView<'a> {
        BodyQueryView {
            body: self.body,
            facts: BodyQueryFacts::Final(self.facts),
        }
    }

    pub fn owner(self) -> BodyOwner {
        self.body.owner()
    }

    pub fn function_owner(self) -> Option<FunctionRef> {
        self.owner().function()
    }

    pub fn owner_module(self) -> ModuleRef {
        self.body.owner_module()
    }

    pub fn fallback_module(self) -> ModuleRef {
        self.body.fallback_module()
    }

    pub fn source(self) -> BodySource {
        self.body.source()
    }

    pub fn source_items(self) -> &'a BodySourceItems {
        self.body.source_items()
    }

    pub fn macro_calls(self) -> &'a [BodyMacroCallData] {
        self.body.macro_calls()
    }

    pub fn param_scope(self) -> ScopeId {
        self.body.param_scope()
    }

    pub fn root_expr(self) -> ExprId {
        self.body.root_expr()
    }

    pub fn function_params(self) -> &'a [FunctionParamData] {
        self.body.function_params()
    }

    pub fn params(self) -> &'a [BindingId] {
        self.body.params()
    }

    pub fn scopes(self) -> &'a [ScopeData] {
        self.body.scopes()
    }

    pub fn bindings(self) -> &'a [BindingData] {
        self.body.bindings()
    }

    pub fn binding_facts(self) -> &'a [BindingFacts] {
        self.facts.bindings.as_slice()
    }

    pub fn pats(self) -> &'a [PatData] {
        self.body.pats()
    }

    pub fn statements(self) -> &'a [StmtData] {
        self.body.statements()
    }

    pub fn exprs(self) -> &'a [ExprData] {
        self.body.exprs()
    }

    pub fn expr_facts(self) -> &'a [ExprFacts] {
        self.facts.exprs.as_slice()
    }

    pub fn binding(self, binding: BindingId) -> Option<&'a BindingData> {
        self.body.binding(binding)
    }

    pub fn binding_fact(self, binding: BindingId) -> Option<&'a BindingFacts> {
        self.facts.bindings.get(binding)
    }

    pub fn pat(self, pat: PatId) -> Option<&'a PatData> {
        self.body.pat(pat)
    }

    pub fn scope(self, scope: ScopeId) -> Option<&'a ScopeData> {
        self.body.scope(scope)
    }

    pub fn scope_for_module(self, body_ref: BodyRef, module: ModuleRef) -> Option<ScopeId> {
        self.body.scope_for_module(body_ref, module)
    }

    pub fn source_item(self, item: ItemTreeId) -> Option<&'a ItemNode> {
        self.body.source_item(item)
    }

    pub fn source_item_source(self, item: ItemTreeId) -> Option<BodySource> {
        self.body.source_item_source(item)
    }

    pub fn source_item_is_written(self, item: ItemTreeId) -> bool {
        self.body.source_item_is_written(item)
    }

    pub fn statement(self, statement: StmtId) -> Option<&'a StmtData> {
        self.body.statement(statement)
    }

    pub fn expr(self, expr: ExprId) -> Option<&'a ExprData> {
        self.body.expr(expr)
    }

    pub fn expr_fact(self, expr: ExprId) -> Option<&'a ExprFacts> {
        self.facts.exprs.get(expr)
    }

    /// Return the durable target selected for this call expression after inference.
    pub fn call_facts(self, expr: ExprId) -> Option<&'a CallFacts> {
        self.facts.call(expr)
    }

    pub fn expr_ty(self, expr: ExprId) -> Option<&'a rg_ty::Ty> {
        self.facts.exprs.get(expr).map(|facts| &facts.ty)
    }

    pub fn expr_declarations(self, body_ref: BodyRef, expr: ExprId) -> Vec<DeclarationRef> {
        self.expr_resolution(expr)
            .map(|resolution| resolution.declarations(body_ref))
            .unwrap_or_default()
    }

    pub(crate) fn expr_resolution(self, expr: ExprId) -> Option<&'a BodyResolution> {
        self.facts.exprs.get(expr).map(|facts| &facts.resolution)
    }

    pub fn binding_ty(self, binding: BindingId) -> Option<&'a rg_ty::Ty> {
        self.facts.bindings.get(binding).map(|facts| &facts.ty)
    }
}

impl<'a> BodyQueryView<'a> {
    /// Join frozen structure and accumulated resolutions with one inference-step type snapshot.
    ///
    /// The snapshot is read-only. Transfer rules write into the separate live inference context,
    /// then the next fixed-point step builds a new query view from the refined slots.
    pub(crate) fn for_inference(
        body: &'a BodyData,
        resolutions: &'a BodyFacts,
        expr_tys: &'a [rg_ty::Ty],
        binding_tys: &'a [rg_ty::Ty],
    ) -> Self {
        debug_assert_eq!(body.exprs().len(), expr_tys.len());
        debug_assert_eq!(body.bindings().len(), binding_tys.len());
        debug_assert!(resolutions.is_aligned_with(body));
        Self {
            body,
            facts: BodyQueryFacts::Inference {
                resolutions,
                expr_tys,
                binding_tys,
            },
        }
    }

    pub(crate) fn structure(self) -> &'a BodyData {
        self.body
    }

    pub(crate) fn expr_ty_unchecked(self, expr: ExprId) -> &'a rg_ty::Ty {
        match self.facts {
            BodyQueryFacts::Final(facts) => &facts.exprs[expr].ty,
            BodyQueryFacts::Inference { expr_tys, .. } => &expr_tys[expr.0],
        }
    }

    pub(crate) fn expr_resolution_unchecked(self, expr: ExprId) -> &'a BodyResolution {
        match self.facts {
            BodyQueryFacts::Final(facts) => &facts.exprs[expr].resolution,
            BodyQueryFacts::Inference { resolutions, .. } => &resolutions.exprs[expr].resolution,
        }
    }

    pub(crate) fn binding_ty_unchecked(self, binding: BindingId) -> &'a rg_ty::Ty {
        match self.facts {
            BodyQueryFacts::Final(facts) => &facts.bindings[binding].ty,
            BodyQueryFacts::Inference { binding_tys, .. } => &binding_tys[binding.0],
        }
    }
}
