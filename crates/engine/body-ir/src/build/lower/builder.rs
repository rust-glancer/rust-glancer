//! Build-only body storage between syntax lowering and binding materialization.

use rg_arena::Arena;
use rg_ir_model::{BindingId, ExprId, ModuleRef, PatId, ScopeId, StmtId};
use rg_item_tree::{ItemNode, ItemTreeId};

use crate::ir::{
    BindingData, BodyData, BodyMacroCallData, BodyOwner, BodySource, BodySourceItems, ExprData,
    FunctionParamData, PatData, ScopeData, StmtData,
};

/// Body shape plus the source ambiguity that must be settled before the shape is frozen.
///
/// The pending arena uses the same temporary binding ids as `body`. Pattern materialization
/// compacts those ids and consumes this value into the immutable `BodyData` stored by the database.
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
pub(super) struct BodyBuilder {
    pub(super) source_items: BodySourceItems,
    pub(super) macro_calls: Vec<BodyMacroCallData>,
    pub(super) scopes: Arena<ScopeId, ScopeData>,
    pub(super) bindings: Arena<BindingId, BindingData>,
    pub(super) pending_binding_resolutions: Arena<BindingId, PendingBindingResolution>,
    pub(super) pats: Arena<PatId, PatData>,
    pub(super) statements: Arena<StmtId, StmtData>,
    pub(super) exprs: Arena<ExprId, ExprData>,
}

impl BodyBuilder {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish(
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

    pub(super) fn alloc_scope(&mut self, parent: Option<ScopeId>) -> ScopeId {
        self.scopes.alloc(ScopeData {
            parent,
            source_items: Vec::new(),
            bindings: Vec::new(),
        })
    }

    pub(super) fn push_macro_call(&mut self, data: BodyMacroCallData) {
        self.macro_calls.push(data);
    }

    /// Some items do not directly belong to a scope, e.g. contents of `impl` block.
    /// These are only indexed by their item ID, but not recorded as a part of the scope.
    pub(super) fn alloc_scopeless_source_item(
        &mut self,
        data: ItemNode,
        source: BodySource,
    ) -> ItemTreeId {
        self.source_items.alloc(data, source)
    }

    /// Items declared within an expression scope are associated with the corresponding scope.
    pub(super) fn alloc_scope_source_item(
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

    pub(super) fn alloc_binding(&mut self, data: BindingData) -> BindingId {
        self.alloc_pending_binding(data, PendingBindingResolution::AlwaysBinding)
    }

    pub(super) fn alloc_pending_binding(
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

    pub(super) fn alloc_pat(&mut self, data: PatData) -> PatId {
        self.pats.alloc(data)
    }

    pub(super) fn alloc_statement(&mut self, data: StmtData) -> StmtId {
        self.statements.alloc(data)
    }

    pub(super) fn alloc_expr(&mut self, data: ExprData) -> ExprId {
        self.exprs.alloc(data)
    }
}
