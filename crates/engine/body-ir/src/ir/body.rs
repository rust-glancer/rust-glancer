use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use rg_arena::Arena;
use rg_ir_model::{
    BindingData, BindingId, BodyData, BodyMacroCallData, BodyOwner, BodyRef, BodySource,
    BodySourceItems, ExprData, ExprId, FunctionParamData, FunctionRef, ModuleRef, PatData, PatId,
    ScopeData, ScopeId, StmtData, StmtId,
    identity::DeclarationRef,
    items::{ItemNode, ItemTreeId},
};

use super::resolved::{BindingFacts, BodyFacts, BodyResolution, ExprFacts};

/// Body storage with model-shaped body data plus pass-derived resolution facts.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ResolvedBodyData {
    pub(crate) body: BodyData,
    pub(crate) facts: BodyFacts,
    pub(crate) pending_binding_resolutions: Arena<BindingId, PendingBindingResolution>,
}

impl ResolvedBodyData {
    pub fn owner(&self) -> BodyOwner {
        self.body.owner()
    }

    pub fn function_owner(&self) -> Option<FunctionRef> {
        self.owner().function()
    }

    pub fn owner_module(&self) -> ModuleRef {
        self.body.owner_module()
    }

    pub fn fallback_module(&self) -> ModuleRef {
        self.body.fallback_module()
    }

    pub fn source(&self) -> BodySource {
        self.body.source()
    }

    pub fn source_items(&self) -> &BodySourceItems {
        self.body.source_items()
    }

    pub fn macro_calls(&self) -> &[BodyMacroCallData] {
        self.body.macro_calls()
    }

    pub fn param_scope(&self) -> ScopeId {
        self.body.param_scope()
    }

    pub fn root_expr(&self) -> ExprId {
        self.body.root_expr()
    }

    pub fn function_params(&self) -> &[FunctionParamData] {
        self.body.function_params()
    }

    pub fn params(&self) -> &[BindingId] {
        self.body.params()
    }

    pub fn scopes(&self) -> &[ScopeData] {
        self.body.scopes()
    }

    pub fn bindings(&self) -> &[BindingData] {
        self.body.bindings()
    }

    pub fn binding_facts(&self) -> &[BindingFacts] {
        self.facts.bindings.as_slice()
    }

    pub fn pats(&self) -> &[PatData] {
        self.body.pats()
    }

    pub fn statements(&self) -> &[StmtData] {
        self.body.statements()
    }

    pub fn exprs(&self) -> &[ExprData] {
        self.body.exprs()
    }

    pub(crate) fn scopes_with_ids(&self) -> impl Iterator<Item = (ScopeId, &ScopeData)> {
        self.body.scopes_with_ids()
    }

    pub(crate) fn exprs_with_ids(&self) -> impl Iterator<Item = (ExprId, &ExprData)> {
        self.body.exprs_with_ids()
    }

    pub fn expr_facts(&self) -> &[ExprFacts] {
        self.facts.exprs.as_slice()
    }

    pub fn binding(&self, binding: BindingId) -> Option<&BindingData> {
        self.body.binding(binding)
    }

    pub(crate) fn binding_unchecked(&self, binding: BindingId) -> &BindingData {
        self.body.binding_unchecked(binding)
    }

    pub fn binding_fact(&self, binding: BindingId) -> Option<&BindingFacts> {
        self.facts.bindings.get(binding)
    }

    pub fn pat(&self, pat: PatId) -> Option<&PatData> {
        self.body.pat(pat)
    }

    pub fn scope(&self, scope: ScopeId) -> Option<&ScopeData> {
        self.body.scope(scope)
    }

    pub fn scope_for_module(&self, body_ref: BodyRef, module: ModuleRef) -> Option<ScopeId> {
        self.body.scope_for_module(body_ref, module)
    }

    pub fn source_item(&self, item: ItemTreeId) -> Option<&ItemNode> {
        self.body.source_item(item)
    }

    pub fn source_item_source(&self, item: ItemTreeId) -> Option<BodySource> {
        self.body.source_item_source(item)
    }

    pub fn source_item_is_written(&self, item: ItemTreeId) -> bool {
        self.body.source_item_is_written(item)
    }

    pub fn statement(&self, statement: StmtId) -> Option<&StmtData> {
        self.body.statement(statement)
    }

    pub(crate) fn statement_unchecked(&self, statement: StmtId) -> &StmtData {
        self.body.statement_unchecked(statement)
    }

    pub fn expr(&self, expr: ExprId) -> Option<&ExprData> {
        self.body.expr(expr)
    }

    pub(crate) fn expr_unchecked(&self, expr: ExprId) -> &ExprData {
        self.body.expr_unchecked(expr)
    }

    pub fn expr_fact(&self, expr: ExprId) -> Option<&ExprFacts> {
        self.facts.exprs.get(expr)
    }

    pub fn expr_ty(&self, expr: ExprId) -> Option<&rg_ty::Ty> {
        self.expr_fact(expr).map(|facts| &facts.ty)
    }

    pub fn expr_declarations(&self, body_ref: BodyRef, expr: ExprId) -> Vec<DeclarationRef> {
        self.expr_fact(expr)
            .map(|facts| facts.resolution.declarations(body_ref))
            .unwrap_or_default()
    }

    pub(crate) fn expr_ty_unchecked(&self, expr: ExprId) -> &rg_ty::Ty {
        &self.facts.exprs[expr].ty
    }

    pub(crate) fn set_expr_ty(&mut self, expr: ExprId, ty: rg_ty::Ty) {
        self.facts.exprs[expr].ty = ty;
    }

    pub(crate) fn expr_resolution(&self, expr: ExprId) -> &BodyResolution {
        &self.facts.exprs[expr].resolution
    }

    pub(crate) fn set_expr_facts(
        &mut self,
        expr: ExprId,
        resolution: BodyResolution,
        ty: rg_ty::Ty,
    ) {
        let facts = &mut self.facts.exprs[expr];
        facts.resolution = resolution;
        facts.ty = ty;
    }

    pub fn binding_ty(&self, binding: BindingId) -> Option<&rg_ty::Ty> {
        self.binding_fact(binding).map(|facts| &facts.ty)
    }

    pub(crate) fn binding_ty_unchecked(&self, binding: BindingId) -> &rg_ty::Ty {
        &self.facts.bindings[binding].ty
    }

    pub(crate) fn set_binding_ty(&mut self, binding: BindingId, ty: rg_ty::Ty) {
        self.facts.bindings[binding].ty = ty;
    }

    /// Resolves pending binding slots into final bindings and rewrites every dependent reference.
    pub(crate) fn compact_bindings(&mut self, active: Vec<bool>) {
        self.body.compact_bindings(&active);

        let mut new_binding_facts = Arena::with_capacity(self.body.bindings().len());
        for (binding_idx, _) in self.body.bindings().iter().enumerate() {
            let new_facts = new_binding_facts.alloc(BindingFacts::default());
            debug_assert_eq!(
                BindingId(binding_idx),
                new_facts,
                "binding facts should mirror materialized binding ids",
            );
        }

        self.facts.bindings = new_binding_facts;
        self.pending_binding_resolutions.clear();
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        owner: BodyOwner,
        owner_module: ModuleRef,
        fallback_module: ModuleRef,
        source: BodySource,
        param_scope: ScopeId,
        root_expr: ExprId,
        function_params: Vec<FunctionParamData>,
        params: Vec<BindingId>,
        builder: BodyBuilder,
    ) -> Self {
        let (body, facts, pending_binding_resolutions) = builder.into_body_data(
            owner,
            owner_module,
            fallback_module,
            source,
            param_scope,
            root_expr,
            function_params,
            params,
        );

        Self {
            body,
            facts,
            pending_binding_resolutions,
        }
    }
}

/// How a lowered binding slot should be treated before final binding materialization.
///
/// Pattern lowering records ambiguous identifiers as slots first. Body resolution then decides
/// whether each slot becomes a real binding or remains a path-pattern use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub(crate) enum PendingBindingResolution {
    AlwaysBinding,
    AmbiguousPattern,
}

/// Mutable store used while one body is being lowered.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct BodyBuilder {
    pub(crate) source_items: BodySourceItems,
    pub(crate) macro_calls: Vec<BodyMacroCallData>,
    pub(crate) scopes: Arena<ScopeId, ScopeData>,
    pub(crate) bindings: Arena<BindingId, BindingData>,
    pub(crate) facts: BodyFacts,
    pub(crate) pending_binding_resolutions: Arena<BindingId, PendingBindingResolution>,
    pub(crate) pats: Arena<PatId, PatData>,
    pub(crate) statements: Arena<StmtId, StmtData>,
    pub(crate) exprs: Arena<ExprId, ExprData>,
}

impl BodyBuilder {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn into_body_data(
        self,
        owner: BodyOwner,
        owner_module: ModuleRef,
        fallback_module: ModuleRef,
        source: BodySource,
        param_scope: ScopeId,
        root_expr: ExprId,
        function_params: Vec<FunctionParamData>,
        params: Vec<BindingId>,
    ) -> (
        BodyData,
        BodyFacts,
        Arena<BindingId, PendingBindingResolution>,
    ) {
        let Self {
            source_items,
            macro_calls,
            scopes,
            bindings,
            facts,
            pending_binding_resolutions,
            pats,
            statements,
            exprs,
        } = self;

        (
            BodyData::new(
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
            facts,
            pending_binding_resolutions,
        )
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
        let facts = self.facts.bindings.alloc(BindingFacts::default());
        debug_assert_eq!(
            binding, facts,
            "binding facts should mirror binding slot ids"
        );
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
        let expr = self.exprs.alloc(data);
        let facts = self.facts.exprs.alloc(ExprFacts::default());
        debug_assert_eq!(
            expr, facts,
            "expression facts should mirror expression slot ids"
        );
        expr
    }
}
