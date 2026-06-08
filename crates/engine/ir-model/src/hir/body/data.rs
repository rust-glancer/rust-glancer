use rg_arena::Arena;
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    BindingId, BodyRef, DefMapRef, ExprId, ModuleRef, PatId, ScopeId, StmtId,
    items::{ItemNode, ItemTreeId},
};
use rg_std::MemorySize;

use super::{
    BindingData, BodyOwner, BodySource, BodySourceItems, ExprData, ExprKind, PatData, PatKind,
    ScopeData, StmtData, StmtKind,
};

/// Model-shaped expression body for a function, const, or static initializer.
///
/// This is the pure body shape: source identity, lexical scopes, and lowered node arenas.
/// Resolution keeps derived facts in separate sidecars owned by the body resolution layer.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct BodyData {
    owner: BodyOwner,
    owner_module: ModuleRef,
    fallback_module: ModuleRef,
    source: BodySource,
    source_items: BodySourceItems,
    param_scope: ScopeId,
    root_expr: ExprId,
    params: Vec<BindingId>,
    scopes: Arena<ScopeId, ScopeData>,
    bindings: Arena<BindingId, BindingData>,
    pats: Arena<PatId, PatData>,
    statements: Arena<StmtId, StmtData>,
    exprs: Arena<ExprId, ExprData>,
}

impl BodyData {
    // Lowering naturally produces these independent body fields at the same boundary. A wrapper
    // object would only move the argument list elsewhere without making the invariant clearer.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        owner: BodyOwner,
        owner_module: ModuleRef,
        fallback_module: ModuleRef,
        source: BodySource,
        source_items: BodySourceItems,
        param_scope: ScopeId,
        root_expr: ExprId,
        params: Vec<BindingId>,
        scopes: Arena<ScopeId, ScopeData>,
        bindings: Arena<BindingId, BindingData>,
        pats: Arena<PatId, PatData>,
        statements: Arena<StmtId, StmtData>,
        exprs: Arena<ExprId, ExprData>,
    ) -> Self {
        Self {
            owner,
            owner_module,
            fallback_module,
            source,
            source_items,
            param_scope,
            root_expr,
            params,
            scopes,
            bindings,
            pats,
            statements,
            exprs,
        }
    }

    pub fn owner(&self) -> BodyOwner {
        self.owner
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

    pub fn pats(&self) -> &[PatData] {
        self.pats.as_slice()
    }

    pub fn statements(&self) -> &[StmtData] {
        self.statements.as_slice()
    }

    pub fn exprs(&self) -> &[ExprData] {
        self.exprs.as_slice()
    }

    pub fn scopes_with_ids(&self) -> impl Iterator<Item = (ScopeId, &ScopeData)> {
        self.scopes.iter_with_ids()
    }

    pub fn exprs_with_ids(&self) -> impl Iterator<Item = (ExprId, &ExprData)> {
        self.exprs.iter_with_ids()
    }

    pub fn binding(&self, binding: BindingId) -> Option<&BindingData> {
        self.bindings.get(binding)
    }

    pub fn binding_unchecked(&self, binding: BindingId) -> &BindingData {
        &self.bindings[binding]
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

    pub fn statement_unchecked(&self, statement: StmtId) -> &StmtData {
        &self.statements[statement]
    }

    pub fn expr(&self, expr: ExprId) -> Option<&ExprData> {
        self.exprs.get(expr)
    }

    pub fn expr_unchecked(&self, expr: ExprId) -> &ExprData {
        &self.exprs[expr]
    }

    /// Resolves pending binding slots into final bindings and rewrites every dependent reference.
    pub fn compact_bindings(&mut self, active: &[bool]) {
        let pending_count = self.bindings.len();
        let mut old_to_new = vec![None; pending_count];
        let mut new_bindings =
            Arena::with_capacity(active.iter().filter(|&&active| active).count());
        for (binding_idx, binding_data) in self.bindings.iter().cloned().enumerate() {
            if !active[binding_idx] {
                continue;
            }

            let new_binding = new_bindings.alloc(binding_data);
            old_to_new[binding_idx] = Some(new_binding);
        }

        // `visible_bindings` stores a count, not an id. The boundary map translates an old pending
        // count into the number of real bindings that remain visible at the same source point.
        let mut boundary_map = Vec::with_capacity(pending_count + 1);
        let mut visible = 0;
        boundary_map.push(visible);
        for is_active in active {
            if *is_active {
                visible += 1;
            }
            boundary_map.push(visible);
        }

        // Lowering stored pending ids in many places: scope binding lists, pattern-owned binding
        // lists, and expression visibility boundaries. They all have to move together or later
        // path lookup will see a different scope than the pattern tree describes.
        rewrite_binding_list(&mut self.params, &old_to_new);
        for scope in self.scopes.iter_mut() {
            rewrite_binding_list(&mut scope.bindings, &old_to_new);
        }
        for statement in self.statements.iter_mut() {
            if let StmtKind::Let { bindings, .. } = &mut statement.kind {
                rewrite_binding_list(bindings, &old_to_new);
            }
        }
        for expr in self.exprs.iter_mut() {
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
        for pat in self.pats.iter_mut() {
            if let PatKind::Binding { binding, .. } = &mut pat.kind
                && let Some(old_binding) = *binding
            {
                *binding = old_to_new.get(old_binding.0).copied().flatten();
            }
        }

        self.bindings = new_bindings;
    }

    pub fn shrink_to_fit(&mut self) {
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
