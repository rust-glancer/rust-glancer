use rg_ir_model::{
    BindingId, BodyRef, ExprId, FunctionRef, ModuleRef, PatId, ScopeId, SourceId, Span, StmtId,
    identity::DeclarationRef,
};
use rg_item_tree::{ItemNode, ItemTreeId};

use super::{
    BindingData, BodyData, BodyMacroCallData, BodyOwner, BodySource, BodySourceItems,
    BodySourceMap, BodySourceStore, ExprData, FunctionParamData, PatData, ScopeData, StmtData,
    facts::{BodyFacts, BodyResolution, CallFacts, ExprFacts},
};

/// Read view joining one prepared body, its inferred facts, and its source locations.
///
/// `expr(id)` reads the lowered node; `expr_ty(id)` and `expr_declarations(...)` read conclusions
/// for that same ID. `source_at(node.source)` supplies the current position. The first two can
/// stay unchanged after a comment moves the expression. The source table must come from the new
/// preparation, so this view borrows each part without copying them into a combined payload.
///
/// Construction expects the dense sidecars to mirror the body's arenas and checks that invariant
/// in debug builds. These facts are published only after inference has removed its live variables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyView<'a> {
    body: &'a BodyData,
    facts: &'a BodyFacts,
    sources: &'a BodySourceStore,
}

impl<'a> BodyView<'a> {
    pub(crate) fn new(
        body: &'a BodyData,
        facts: &'a BodyFacts,
        sources: &'a BodySourceStore,
    ) -> Self {
        debug_assert!(
            facts.is_aligned_with(body),
            "body facts should mirror body binding and expression ids",
        );
        Self {
            body,
            facts,
            sources,
        }
    }

    pub fn structure(self) -> &'a BodyData {
        self.body
    }

    pub fn sources(self) -> &'a BodySourceStore {
        self.sources
    }

    pub fn source_at(self, id: SourceId) -> BodySource {
        self.sources.source(id)
    }

    pub fn span(self, id: SourceId) -> Span {
        self.sources.span(id)
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
        self.sources.source(self.body.source())
    }

    pub fn source_items(self) -> &'a BodySourceItems {
        self.body.source_items()
    }

    pub fn macro_calls(self) -> &'a [BodyMacroCallData] {
        self.body.macro_calls()
    }

    pub fn source_map(self) -> &'a BodySourceMap {
        self.sources.provenance()
    }

    pub fn contains_source_span(self, source: BodySource, span: Span) -> bool {
        self.source_map().is_macro_argument(span)
            || source.is_written()
                && !self
                    .macro_calls()
                    .iter()
                    .any(|call| self.source_at(call.source).span.contains_span(span))
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
        self.body
            .source_item_source(item)
            .map(|id| self.source_at(id))
    }

    pub fn source_item_is_written(self, item: ItemTreeId) -> bool {
        self.source_item_source(item)
            .is_none_or(|source| source.is_written())
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
