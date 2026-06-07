use wincode::{SchemaRead, SchemaWrite};

use rg_arena::Arena;
use rg_ir_model::{
    BindingId, BodyId, BodyOwner, BodyRef, BodySource, DefMapRef, ExprId, FunctionRef, ModuleRef,
    PatId, ScopeId, StmtId,
};
use rg_ir_storage::{DefMap, ItemStore};
use rg_item_tree::{ItemNode, ItemTreeId};
use rg_memsize::MemorySize;
use rg_parse::TargetId;

use super::{
    expr::ExprData,
    pat::PatData,
    resolved::{BindingFacts, BodyResolution, ExprFacts},
    source_items::BodySourceItems,
    stmt::{BindingData, PendingBindingResolution, StmtData},
};

/// Coarse totals for reporting that the Body IR phase produced useful data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, MemorySize)]
pub struct BodyIrStats {
    pub target_count: usize,
    pub built_target_count: usize,
    pub skipped_target_count: usize,
    pub body_count: usize,
    pub scope_count: usize,
    pub binding_count: usize,
    pub statement_count: usize,
    pub expression_count: usize,
}

/// Lowered bodies for all targets inside one parsed package.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize)]
pub struct PackageBodies {
    pub(crate) targets: Arena<TargetId, TargetBodies>,
}

impl PackageBodies {
    pub(crate) fn new(targets: Vec<TargetBodies>) -> Self {
        Self {
            targets: Arena::from_vec(targets),
        }
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.targets.shrink_to_fit();
        for target in self.targets.iter_mut() {
            target.shrink_to_fit();
        }
    }

    pub fn targets(&self) -> &[TargetBodies] {
        self.targets.as_slice()
    }

    pub fn target(&self, target: TargetId) -> Option<&TargetBodies> {
        self.targets.get(target)
    }
}

impl PackageBodies {
    pub(crate) fn targets_mut(&mut self) -> &mut [TargetBodies] {
        self.targets.as_mut_slice()
    }
}

/// Lowered bodies for one target.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct TargetBodies {
    pub(crate) status: TargetBodiesStatus,
    pub(crate) bodies: Arena<BodyId, BodyData>,
    pub(crate) body_local_items: Arena<BodyId, BodyLocalItems>,
}

impl TargetBodies {
    pub(crate) fn new() -> Self {
        Self {
            status: TargetBodiesStatus::Built,
            bodies: Arena::new(),
            body_local_items: Arena::new(),
        }
    }

    pub(crate) fn skipped() -> Self {
        Self {
            status: TargetBodiesStatus::Skipped,
            bodies: Arena::new(),
            body_local_items: Arena::new(),
        }
    }

    pub fn status(&self) -> TargetBodiesStatus {
        self.status
    }

    pub fn body(&self, body: BodyId) -> Option<&BodyData> {
        self.bodies.get(body)
    }

    pub fn body_local_items(&self, body: BodyId) -> Option<&BodyLocalItems> {
        self.body_local_items.get(body)
    }

    pub fn body_def_map(&self, body: BodyId) -> Option<&DefMap> {
        self.body_local_items(body).map(|items| &items.def_map)
    }

    pub fn body_item_store(&self, body: BodyId) -> Option<&ItemStore> {
        self.body_local_items(body).map(|items| &items.item_store)
    }

    pub fn bodies(&self) -> &[BodyData] {
        self.bodies.as_slice()
    }

    pub(crate) fn alloc_body(&mut self, data: BodyData) -> BodyId {
        self.bodies.alloc(data)
    }

    pub(crate) fn set_body_local_items(&mut self, items: Vec<BodyLocalItems>) {
        debug_assert_eq!(
            self.bodies.len(),
            items.len(),
            "every built body should have finalized body-local items"
        );
        self.body_local_items = Arena::from_vec(items);
    }

    pub(crate) fn bodies_mut(&mut self) -> &mut [BodyData] {
        self.bodies.as_mut_slice()
    }

    fn shrink_to_fit(&mut self) {
        self.bodies.shrink_to_fit();
        for body in self.bodies.iter_mut() {
            body.shrink_to_fit();
        }
        self.body_local_items.shrink_to_fit();
        for items in self.body_local_items.iter_mut() {
            items.shrink_to_fit();
        }
    }
}

/// Whether one target's bodies were eagerly lowered.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, derive_more::Display, SchemaRead, SchemaWrite, MemorySize,
)]
#[memsize(leaf)]
pub enum TargetBodiesStatus {
    #[display("built")]
    Built,
    #[display("skipped")]
    Skipped,
}

/// Finalized body-local DefMap and semantic-shaped item facts for one body.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct BodyLocalItems {
    pub(crate) def_map: DefMap,
    pub(crate) item_store: ItemStore,
}

impl BodyLocalItems {
    pub(crate) fn new(def_map: DefMap, item_store: ItemStore) -> Self {
        Self {
            def_map,
            item_store,
        }
    }

    pub fn def_map(&self) -> &DefMap {
        &self.def_map
    }

    pub fn item_store(&self) -> &ItemStore {
        &self.item_store
    }

    fn shrink_to_fit(&mut self) {
        self.def_map.shrink_to_fit();
        self.item_store.shrink_to_fit();
    }
}

/// Lowered expression body for a function, const, or static initializer.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct BodyData {
    pub(crate) owner: BodyOwner,
    pub(crate) owner_module: ModuleRef,
    pub(crate) fallback_module: ModuleRef,
    pub(crate) source: BodySource,
    pub(crate) source_items: BodySourceItems,
    pub(crate) param_scope: ScopeId,
    pub(crate) root_expr: ExprId,
    pub(crate) params: Vec<BindingId>,
    pub(crate) scopes: Arena<ScopeId, ScopeData>,
    pub(crate) bindings: Arena<BindingId, BindingData>,
    pub(crate) binding_facts: Arena<BindingId, BindingFacts>,
    pub(crate) pending_binding_resolutions: Arena<BindingId, PendingBindingResolution>,
    pub(crate) pats: Arena<PatId, PatData>,
    pub(crate) statements: Arena<StmtId, StmtData>,
    pub(crate) exprs: Arena<ExprId, ExprData>,
    pub(crate) expr_facts: Arena<ExprId, ExprFacts>,
}

impl BodyData {
    pub fn owner(&self) -> BodyOwner {
        self.owner
    }

    pub fn function_owner(&self) -> Option<FunctionRef> {
        self.owner.function()
    }

    pub fn owner_module(&self) -> ModuleRef {
        self.owner_module
    }

    pub fn fallback_module(&self) -> ModuleRef {
        self.fallback_module
    }

    pub fn source(&self) -> BodySource {
        self.source
    }

    pub fn source_items(&self) -> &BodySourceItems {
        &self.source_items
    }

    pub fn param_scope(&self) -> ScopeId {
        self.param_scope
    }

    pub fn root_expr(&self) -> ExprId {
        self.root_expr
    }

    pub fn params(&self) -> &[BindingId] {
        &self.params
    }

    pub fn scopes(&self) -> &[ScopeData] {
        self.scopes.as_slice()
    }

    pub fn bindings(&self) -> &[BindingData] {
        self.bindings.as_slice()
    }

    pub fn binding_facts(&self) -> &[BindingFacts] {
        self.binding_facts.as_slice()
    }

    pub fn pats(&self) -> &[PatData] {
        self.pats.as_slice()
    }

    pub fn statements(&self) -> &[StmtData] {
        self.statements.as_slice()
    }

    pub fn exprs(&self) -> &[ExprData] {
        self.exprs.as_slice()
    }

    pub fn expr_facts(&self) -> &[ExprFacts] {
        self.expr_facts.as_slice()
    }

    pub fn binding(&self, binding: BindingId) -> Option<&BindingData> {
        self.bindings.get(binding)
    }

    pub fn binding_fact(&self, binding: BindingId) -> Option<&BindingFacts> {
        self.binding_facts.get(binding)
    }

    pub fn pat(&self, pat: PatId) -> Option<&PatData> {
        self.pats.get(pat)
    }

    pub fn scope(&self, scope: ScopeId) -> Option<&ScopeData> {
        self.scopes.get(scope)
    }

    pub fn scope_for_module(&self, body_ref: BodyRef, module: ModuleRef) -> Option<ScopeId> {
        if module.origin != DefMapRef::Body(body_ref) {
            return None;
        }

        // Body DefMaps allocate synthetic scope modules first, in `ScopeId` order. Inline named
        // modules may have ids after that prefix, so the arena lookup is the invariant check.
        let scope = ScopeId(module.module.0);
        self.scope(scope).map(|_| scope)
    }

    pub fn source_item(&self, item: ItemTreeId) -> Option<&ItemNode> {
        self.source_items.item(item)
    }

    pub fn statement(&self, statement: StmtId) -> Option<&StmtData> {
        self.statements.get(statement)
    }

    pub fn expr(&self, expr: ExprId) -> Option<&ExprData> {
        self.exprs.get(expr)
    }

    pub fn expr_fact(&self, expr: ExprId) -> Option<&ExprFacts> {
        self.expr_facts.get(expr)
    }

    pub fn expr_ty(&self, expr: ExprId) -> Option<&rg_ty::Ty> {
        self.expr_fact(expr).map(|facts| &facts.ty)
    }

    pub(crate) fn expr_ty_unchecked(&self, expr: ExprId) -> &rg_ty::Ty {
        &self.expr_facts[expr].ty
    }

    pub(crate) fn set_expr_ty(&mut self, expr: ExprId, ty: rg_ty::Ty) {
        self.expr_facts[expr].ty = ty;
    }

    pub(crate) fn expr_resolution(&self, expr: ExprId) -> &BodyResolution {
        &self.expr_facts[expr].resolution
    }

    pub(crate) fn set_expr_facts(
        &mut self,
        expr: ExprId,
        resolution: BodyResolution,
        ty: rg_ty::Ty,
    ) {
        let facts = &mut self.expr_facts[expr];
        facts.resolution = resolution;
        facts.ty = ty;
    }

    pub fn binding_ty(&self, binding: BindingId) -> Option<&rg_ty::Ty> {
        self.binding_fact(binding).map(|facts| &facts.ty)
    }

    pub(crate) fn binding_ty_unchecked(&self, binding: BindingId) -> &rg_ty::Ty {
        &self.binding_facts[binding].ty
    }

    pub(crate) fn set_binding_ty(&mut self, binding: BindingId, ty: rg_ty::Ty) {
        self.binding_facts[binding].ty = ty;
    }

    // Lowering naturally produces these independent body facts at the same boundary. A wrapper
    // object would only move the argument list elsewhere without making the invariant clearer.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        owner: BodyOwner,
        owner_module: ModuleRef,
        fallback_module: ModuleRef,
        source: BodySource,
        param_scope: ScopeId,
        root_expr: ExprId,
        params: Vec<BindingId>,
        builder: BodyBuilder,
    ) -> Self {
        Self {
            owner,
            owner_module,
            fallback_module,
            source,
            source_items: builder.source_items,
            param_scope,
            root_expr,
            params,
            scopes: builder.scopes,
            bindings: builder.bindings,
            binding_facts: builder.binding_facts,
            pending_binding_resolutions: builder.pending_binding_resolutions,
            pats: builder.pats,
            statements: builder.statements,
            exprs: builder.exprs,
            expr_facts: builder.expr_facts,
        }
    }

    fn shrink_to_fit(&mut self) {
        self.params.shrink_to_fit();
        self.source_items.shrink_to_fit();
        self.scopes.shrink_to_fit();
        for scope in self.scopes.iter_mut() {
            scope.shrink_to_fit();
        }
        self.bindings.shrink_to_fit();
        for binding in self.bindings.iter_mut() {
            binding.shrink_to_fit();
        }
        self.binding_facts.shrink_to_fit();
        for facts in self.binding_facts.iter_mut() {
            facts.shrink_to_fit();
        }
        self.pending_binding_resolutions.shrink_to_fit();
        self.pats.shrink_to_fit();
        for pat in self.pats.iter_mut() {
            pat.shrink_to_fit();
        }
        self.statements.shrink_to_fit();
        for statement in self.statements.iter_mut() {
            statement.shrink_to_fit();
        }
        self.exprs.shrink_to_fit();
        for expr in self.exprs.iter_mut() {
            expr.shrink_to_fit();
        }
        self.expr_facts.shrink_to_fit();
        for facts in self.expr_facts.iter_mut() {
            facts.shrink_to_fit();
        }
    }
}

/// Mutable store used while one body is being lowered.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct BodyBuilder {
    pub(crate) source_items: BodySourceItems,
    pub(crate) scopes: Arena<ScopeId, ScopeData>,
    pub(crate) bindings: Arena<BindingId, BindingData>,
    pub(crate) binding_facts: Arena<BindingId, BindingFacts>,
    pub(crate) pending_binding_resolutions: Arena<BindingId, PendingBindingResolution>,
    pub(crate) pats: Arena<PatId, PatData>,
    pub(crate) statements: Arena<StmtId, StmtData>,
    pub(crate) exprs: Arena<ExprId, ExprData>,
    pub(crate) expr_facts: Arena<ExprId, ExprFacts>,
}

impl BodyBuilder {
    pub(crate) fn alloc_scope(&mut self, parent: Option<ScopeId>) -> ScopeId {
        self.scopes.alloc(ScopeData {
            parent,
            source_items: Vec::new(),
            bindings: Vec::new(),
        })
    }

    /// Some items do not directly belong to a scope, e.g. contents of `impl` block.
    /// These are only indexed by their item ID, but not recorded as a part of the scope.
    pub(crate) fn alloc_scopeless_source_item(&mut self, data: ItemNode) -> ItemTreeId {
        self.source_items.alloc(data)
    }

    /// Items declared within an expression scope are associated with the corresponding scope.
    pub(crate) fn alloc_scope_source_item(&mut self, scope: ScopeId, data: ItemNode) -> ItemTreeId {
        let item = self.alloc_scopeless_source_item(data);
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
        let facts = self.binding_facts.alloc(BindingFacts::default());
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
        let facts = self.expr_facts.alloc(ExprFacts::default());
        debug_assert_eq!(
            expr, facts,
            "expression facts should mirror expression slot ids"
        );
        expr
    }
}

/// One lexical scope.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct ScopeData {
    pub parent: Option<ScopeId>,
    pub source_items: Vec<ItemTreeId>,
    pub bindings: Vec<BindingId>,
}

impl ScopeData {
    fn shrink_to_fit(&mut self) {
        self.source_items.shrink_to_fit();
        self.bindings.shrink_to_fit();
    }
}
