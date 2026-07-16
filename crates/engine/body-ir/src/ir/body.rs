use rg_arena::Arena;
use rg_ir_model::{
    BindingData, BindingId, BodyData, BodyMacroCallData, BodyOwner, BodyRef, BodySource,
    BodySourceItems, ExprData, ExprId, FunctionParamData, FunctionRef, ModuleRef, PatData, PatId,
    ScopeData, ScopeId, StmtData, StmtId,
    identity::DeclarationRef,
    items::{ItemNode, ItemTreeId},
};

use super::resolved::{BindingFacts, BodyFacts, BodyResolution, CallFacts, ExprFacts};

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

    pub(crate) fn exprs_with_ids(self) -> impl Iterator<Item = (ExprId, &'a ExprData)> {
        self.body.exprs_with_ids()
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

/// Body shape plus the source ambiguity that must be settled before the shape is frozen.
///
/// This type exists only inside the build pipeline. The pending arena uses the same temporary
/// binding ids as `body`; materialization compacts those ids and consumes this value into the
/// immutable `BodyData` stored by the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoweredBodyData {
    body: BodyData,
    pending_binding_resolutions: Arena<BindingId, PendingBindingResolution>,
}

impl LoweredBodyData {
    pub(crate) fn body(&self) -> &BodyData {
        &self.body
    }

    pub(crate) fn pending_binding_resolution(
        &self,
        binding: BindingId,
    ) -> PendingBindingResolution {
        self.pending_binding_resolutions[binding]
    }

    pub(crate) fn has_pending_bindings(&self) -> bool {
        !self.pending_binding_resolutions.is_empty()
    }

    pub(crate) fn compact_bindings(&mut self, active: &[bool]) {
        self.body.compact_bindings(active);
        self.pending_binding_resolutions.clear();
    }

    pub(crate) fn into_body(self) -> BodyData {
        debug_assert!(
            self.pending_binding_resolutions.is_empty(),
            "pending binding candidates should be materialized before BodyData is frozen",
        );
        self.body
    }
}

/// How a lowered binding slot should be treated before final binding materialization.
///
/// Pattern lowering records ambiguous identifiers as slots first. The final structural build step
/// decides whether each slot becomes a real binding or remains a path-pattern use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingBindingResolution {
    AlwaysBinding,
    AmbiguousPattern,
}

/// Mutable structural store used while one body is being lowered.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct BodyBuilder {
    pub(crate) source_items: BodySourceItems,
    pub(crate) macro_calls: Vec<BodyMacroCallData>,
    pub(crate) scopes: Arena<ScopeId, ScopeData>,
    pub(crate) bindings: Arena<BindingId, BindingData>,
    pub(crate) pending_binding_resolutions: Arena<BindingId, PendingBindingResolution>,
    pub(crate) pats: Arena<PatId, PatData>,
    pub(crate) statements: Arena<StmtId, StmtData>,
    pub(crate) exprs: Arena<ExprId, ExprData>,
}

impl BodyBuilder {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn finish(
        self,
        owner: BodyOwner,
        owner_module: ModuleRef,
        fallback_module: ModuleRef,
        source: BodySource,
        param_scope: ScopeId,
        root_expr: ExprId,
        function_params: Vec<FunctionParamData>,
        params: Vec<BindingId>,
    ) -> LoweredBodyData {
        let Self {
            source_items,
            macro_calls,
            scopes,
            bindings,
            pending_binding_resolutions,
            pats,
            statements,
            exprs,
        } = self;

        LoweredBodyData {
            body: BodyData::new(
                owner,
                owner_module,
                fallback_module,
                source,
                source_items,
                macro_calls,
                param_scope,
                root_expr,
                function_params,
                params,
                scopes,
                bindings,
                pats,
                statements,
                exprs,
            ),
            pending_binding_resolutions,
        }
    }

    pub(crate) fn alloc_scope(&mut self, parent: Option<ScopeId>) -> ScopeId {
        self.scopes.alloc(ScopeData {
            parent,
            source_items: Vec::new(),
            bindings: Vec::new(),
        })
    }

    pub(crate) fn push_macro_call(&mut self, data: BodyMacroCallData) {
        self.macro_calls.push(data);
    }

    /// Some items do not directly belong to a scope, e.g. contents of `impl` block.
    /// These are only indexed by their item ID, but not recorded as a part of the scope.
    pub(crate) fn alloc_scopeless_source_item(
        &mut self,
        data: ItemNode,
        source: BodySource,
    ) -> ItemTreeId {
        self.source_items.alloc(data, source)
    }

    /// Items declared within an expression scope are associated with the corresponding scope.
    pub(crate) fn alloc_scope_source_item(
        &mut self,
        scope: ScopeId,
        data: ItemNode,
        source: BodySource,
    ) -> ItemTreeId {
        let item = self.alloc_scopeless_source_item(data, source);
        self.scopes
            .get_mut(scope)
            .expect("source item scope should exist while lowering body")
            .source_items
            .push(item);
        item
    }

    pub(crate) fn alloc_binding(&mut self, data: BindingData) -> BindingId {
        self.alloc_pending_binding(data, PendingBindingResolution::AlwaysBinding)
    }

    pub(crate) fn alloc_pending_binding(
        &mut self,
        data: BindingData,
        resolution: PendingBindingResolution,
    ) -> BindingId {
        let scope = data.scope;
        let binding = self.bindings.alloc(data);
        let resolution_id = self.pending_binding_resolutions.alloc(resolution);
        debug_assert_eq!(
            binding, resolution_id,
            "pending binding resolution should mirror binding slot ids"
        );
        self.scopes
            .get_mut(scope)
            .expect("binding scope should exist while lowering body")
            .bindings
            .push(binding);
        binding
    }

    pub(crate) fn alloc_pat(&mut self, data: PatData) -> PatId {
        self.pats.alloc(data)
    }

    pub(crate) fn alloc_statement(&mut self, data: StmtData) -> StmtId {
        self.statements.alloc(data)
    }

    pub(crate) fn alloc_expr(&mut self, data: ExprData) -> ExprId {
        self.exprs.alloc(data)
    }
}
