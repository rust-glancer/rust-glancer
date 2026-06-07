use wincode::{SchemaRead, SchemaWrite};

use rg_arena::Arena;
use rg_ir_model::{
    BindingData, BindingId, BodyOwner, BodyRef, BodySource, DefMapRef, ExprData, ExprId, ExprKind,
    FunctionRef, ModuleRef, PatData, PatId, PatKind, ScopeData, ScopeId, StmtData, StmtId,
    StmtKind,
    items::{ItemNode, ItemTreeId},
};
use rg_memsize::MemorySize;

use super::{
    resolved::{BindingFacts, BodyFacts, BodyResolution, ExprFacts},
    source_items::BodySourceItems,
};

/// Model-shaped expression body for a function, const, or static initializer.
///
/// This is the pure body shape: source identity, lexical scopes, and lowered node arenas.
/// Resolution keeps derived facts in separate sidecars owned by `ResolvedBodyData`.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub(crate) struct BodyData {
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
    pub(crate) pats: Arena<PatId, PatData>,
    pub(crate) statements: Arena<StmtId, StmtData>,
    pub(crate) exprs: Arena<ExprId, ExprData>,
}

impl BodyData {
    // Lowering naturally produces these independent body fields at the same boundary. A wrapper
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
        builder: &mut BodyBuilder,
    ) -> Self {
        Self {
            owner,
            owner_module,
            fallback_module,
            source,
            source_items: std::mem::take(&mut builder.source_items),
            param_scope,
            root_expr,
            params,
            scopes: std::mem::take(&mut builder.scopes),
            bindings: std::mem::take(&mut builder.bindings),
            pats: std::mem::take(&mut builder.pats),
            statements: std::mem::take(&mut builder.statements),
            exprs: std::mem::take(&mut builder.exprs),
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
    }
}

/// Body storage with model-shaped body data plus pass-derived resolution facts.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct ResolvedBodyData {
    pub(crate) body: BodyData,
    pub(crate) facts: BodyFacts,
    pub(crate) pending_binding_resolutions: Arena<BindingId, PendingBindingResolution>,
}

impl ResolvedBodyData {
    pub fn owner(&self) -> BodyOwner {
        self.body.owner
    }

    pub fn function_owner(&self) -> Option<FunctionRef> {
        self.owner().function()
    }

    pub fn owner_module(&self) -> ModuleRef {
        self.body.owner_module
    }

    pub fn fallback_module(&self) -> ModuleRef {
        self.body.fallback_module
    }

    pub fn source(&self) -> BodySource {
        self.body.source
    }

    pub fn source_items(&self) -> &BodySourceItems {
        &self.body.source_items
    }

    pub fn param_scope(&self) -> ScopeId {
        self.body.param_scope
    }

    pub fn root_expr(&self) -> ExprId {
        self.body.root_expr
    }

    pub fn params(&self) -> &[BindingId] {
        &self.body.params
    }

    pub fn scopes(&self) -> &[ScopeData] {
        self.body.scopes.as_slice()
    }

    pub fn bindings(&self) -> &[BindingData] {
        self.body.bindings.as_slice()
    }

    pub fn binding_facts(&self) -> &[BindingFacts] {
        self.facts.bindings.as_slice()
    }

    pub fn pats(&self) -> &[PatData] {
        self.body.pats.as_slice()
    }

    pub fn statements(&self) -> &[StmtData] {
        self.body.statements.as_slice()
    }

    pub fn exprs(&self) -> &[ExprData] {
        self.body.exprs.as_slice()
    }

    pub(crate) fn scopes_with_ids(&self) -> impl Iterator<Item = (ScopeId, &ScopeData)> {
        self.body.scopes.iter_with_ids()
    }

    pub(crate) fn exprs_with_ids(&self) -> impl Iterator<Item = (ExprId, &ExprData)> {
        self.body.exprs.iter_with_ids()
    }

    pub fn expr_facts(&self) -> &[ExprFacts] {
        self.facts.exprs.as_slice()
    }

    pub fn binding(&self, binding: BindingId) -> Option<&BindingData> {
        self.body.bindings.get(binding)
    }

    pub(crate) fn binding_unchecked(&self, binding: BindingId) -> &BindingData {
        &self.body.bindings[binding]
    }

    pub fn binding_fact(&self, binding: BindingId) -> Option<&BindingFacts> {
        self.facts.bindings.get(binding)
    }

    pub fn pat(&self, pat: PatId) -> Option<&PatData> {
        self.body.pats.get(pat)
    }

    pub fn scope(&self, scope: ScopeId) -> Option<&ScopeData> {
        self.body.scopes.get(scope)
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
        self.body.source_items.item(item)
    }

    pub fn statement(&self, statement: StmtId) -> Option<&StmtData> {
        self.body.statements.get(statement)
    }

    pub(crate) fn statement_unchecked(&self, statement: StmtId) -> &StmtData {
        &self.body.statements[statement]
    }

    pub fn expr(&self, expr: ExprId) -> Option<&ExprData> {
        self.body.exprs.get(expr)
    }

    pub(crate) fn expr_unchecked(&self, expr: ExprId) -> &ExprData {
        &self.body.exprs[expr]
    }

    pub fn expr_fact(&self, expr: ExprId) -> Option<&ExprFacts> {
        self.facts.exprs.get(expr)
    }

    pub fn expr_ty(&self, expr: ExprId) -> Option<&rg_ty::Ty> {
        self.expr_fact(expr).map(|facts| &facts.ty)
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
        let pending_count = self.body.bindings.len();
        let mut old_to_new = vec![None; pending_count];
        let mut new_bindings =
            Arena::with_capacity(active.iter().filter(|&&active| active).count());
        let mut new_binding_facts =
            Arena::with_capacity(active.iter().filter(|&&active| active).count());
        for (binding_idx, binding_data) in self.body.bindings.iter().cloned().enumerate() {
            if !active[binding_idx] {
                continue;
            }

            let new_binding = new_bindings.alloc(binding_data);
            let new_facts = new_binding_facts.alloc(BindingFacts::default());
            debug_assert_eq!(
                new_binding, new_facts,
                "binding facts should mirror materialized binding ids",
            );
            old_to_new[binding_idx] = Some(new_binding);
        }

        // `visible_bindings` stores a count, not an id. The boundary map translates an old pending
        // count into the number of real bindings that remain visible at the same source point.
        let mut boundary_map = Vec::with_capacity(pending_count + 1);
        let mut visible = 0;
        boundary_map.push(visible);
        for is_active in &active {
            if *is_active {
                visible += 1;
            }
            boundary_map.push(visible);
        }

        // Lowering stored pending ids in many places: scope binding lists, pattern-owned binding
        // lists, and expression visibility boundaries. They all have to move together or later
        // path lookup will see a different scope than the pattern tree describes.
        rewrite_binding_list(&mut self.body.params, &old_to_new);
        for scope in self.body.scopes.iter_mut() {
            rewrite_binding_list(&mut scope.bindings, &old_to_new);
        }
        for statement in self.body.statements.iter_mut() {
            if let StmtKind::Let { bindings, .. } = &mut statement.kind {
                rewrite_binding_list(bindings, &old_to_new);
            }
        }
        for expr in self.body.exprs.iter_mut() {
            expr.visible_bindings = boundary_map
                .get(expr.visible_bindings)
                .copied()
                .unwrap_or(visible);

            match &mut expr.kind {
                ExprKind::Let { bindings, .. } | ExprKind::For { bindings, .. } => {
                    rewrite_binding_list(bindings, &old_to_new);
                }
                ExprKind::Closure { params, .. } => {
                    for param in params {
                        rewrite_binding_list(&mut param.bindings, &old_to_new);
                    }
                }
                _ => {}
            }
        }
        for pat in self.body.pats.iter_mut() {
            if let PatKind::Binding { binding, .. } = &mut pat.kind
                && let Some(old_binding) = *binding
            {
                *binding = old_to_new.get(old_binding.0).copied().flatten();
            }
        }

        self.body.bindings = new_bindings;
        self.facts.bindings = new_binding_facts;
        self.pending_binding_resolutions.clear();
    }

    pub(crate) fn new(
        owner: BodyOwner,
        owner_module: ModuleRef,
        fallback_module: ModuleRef,
        source: BodySource,
        param_scope: ScopeId,
        root_expr: ExprId,
        params: Vec<BindingId>,
        mut builder: BodyBuilder,
    ) -> Self {
        Self {
            body: BodyData::new(
                owner,
                owner_module,
                fallback_module,
                source,
                param_scope,
                root_expr,
                params,
                &mut builder,
            ),
            facts: builder.facts,
            pending_binding_resolutions: builder.pending_binding_resolutions,
        }
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.body.shrink_to_fit();
        self.facts.shrink_to_fit();
        self.pending_binding_resolutions.shrink_to_fit();
    }
}

fn rewrite_binding_list(bindings: &mut Vec<BindingId>, old_to_new: &[Option<BindingId>]) {
    let mut rewritten = Vec::with_capacity(bindings.len());
    for binding in bindings.iter().copied() {
        let Some(Some(new_binding)) = old_to_new.get(binding.0) else {
            continue;
        };
        if !rewritten.contains(new_binding) {
            rewritten.push(*new_binding);
        }
    }
    *bindings = rewritten;
}

/// How a lowered binding slot should be treated before final binding materialization.
///
/// Pattern lowering records ambiguous identifiers as slots first. Body resolution then decides
/// whether each slot becomes a real binding or remains a path-pattern use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
#[memsize(leaf)]
pub(crate) enum PendingBindingResolution {
    AlwaysBinding,
    AmbiguousPattern,
}

/// Mutable store used while one body is being lowered.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct BodyBuilder {
    pub(crate) source_items: BodySourceItems,
    pub(crate) scopes: Arena<ScopeId, ScopeData>,
    pub(crate) bindings: Arena<BindingId, BindingData>,
    pub(crate) facts: BodyFacts,
    pub(crate) pending_binding_resolutions: Arena<BindingId, PendingBindingResolution>,
    pub(crate) pats: Arena<PatId, PatData>,
    pub(crate) statements: Arena<StmtId, StmtData>,
    pub(crate) exprs: Arena<ExprId, ExprData>,
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
