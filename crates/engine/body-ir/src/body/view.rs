use rg_ir_model::{
    BindingId, BodyRef, ExprId, FunctionRef, ModuleRef, PatId, ScopeId, StmtId,
    identity::DeclarationRef,
};
use rg_item_tree::{ItemNode, ItemTreeId};

use super::{
    BindingData, BodyData, BodyMacroCallData, BodyOwner, BodySource, BodySourceItems, ExprData,
    FunctionParamData, PatData, ScopeData, StmtData,
    facts::{BodyFacts, BodyResolution, CallFacts, ExprFacts},
};

/// Finalized read view over one structural body and its persisted semantic sidecar.
///
/// `BodyData` and `BodyFacts` have separate owners so structural IR can be built and frozen before
/// resolution starts. Readers normally need both: `expr(id)` reads the lowered node, while
/// `expr_ty(id)` and `expr_declarations(...)` read conclusions for that same id. This view joins
/// the two without creating a third owning representation.
///
/// Construction expects the dense sidecars to mirror the body's arenas and checks that invariant
/// in debug builds. These facts are published only after inference has removed its live variables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyView<'a> {
    body: &'a BodyData,
    facts: &'a BodyFacts,
}

impl<'a> BodyView<'a> {
    pub(crate) fn new(body: &'a BodyData, facts: &'a BodyFacts) -> Self {
        debug_assert!(
            facts.is_aligned_with(body),
            "body facts should mirror body binding and expression ids",
        );
        Self { body, facts }
    }

    pub fn structure(self) -> &'a BodyData {
        self.body
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
        self.facts.bindings.get(binding)
    }
}
