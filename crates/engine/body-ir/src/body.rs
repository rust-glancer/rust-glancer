use rg_arena::Arena;
use rg_parse::{FileId, Span, TargetId};
use rg_semantic_ir::{FunctionId, FunctionRef};

use crate::{
    expr::ExprData,
    ids::{
        BindingId, BodyFunctionId, BodyFunctionRef, BodyId, BodyImplId, BodyItemId, BodyItemRef,
        BodyRef, ExprId, PatId, ScopeId, StmtId,
    },
    item::{BodyFunctionData, BodyImplData, BodyItemData},
    pat::PatData,
    stmt::{BindingData, StmtData},
};

/// Coarse totals for reporting that the Body IR phase produced useful data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BodyIrStats {
    pub target_count: usize,
    pub built_target_count: usize,
    pub skipped_target_count: usize,
    pub body_count: usize,
    pub scope_count: usize,
    pub local_item_count: usize,
    pub local_impl_count: usize,
    pub local_function_count: usize,
    pub binding_count: usize,
    pub statement_count: usize,
    pub expression_count: usize,
}

/// Lowered bodies for all targets inside one parsed package.
#[derive(
    Debug, Clone, PartialEq, Eq, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct PackageBodies {
    pub(crate) targets: Arena<TargetId, TargetBodies>,
}

impl PackageBodies {
    pub(super) fn new(targets: Vec<TargetBodies>) -> Self {
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
    pub(super) fn targets_mut(&mut self) -> &mut [TargetBodies] {
        self.targets.as_mut_slice()
    }
}

/// Lowered bodies for one target.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct TargetBodies {
    pub(crate) status: TargetBodiesStatus,
    pub(crate) function_bodies: Arena<FunctionId, Option<BodyId>>,
    pub(crate) bodies: Arena<BodyId, BodyData>,
}

impl TargetBodies {
    pub(super) fn new(function_count: usize) -> Self {
        Self {
            status: TargetBodiesStatus::Built,
            function_bodies: {
                let mut function_bodies = Arena::new();
                function_bodies.resize_with(function_count, || None);
                function_bodies
            },
            bodies: Arena::new(),
        }
    }

    pub(super) fn skipped(function_count: usize) -> Self {
        Self {
            status: TargetBodiesStatus::Skipped,
            function_bodies: {
                let mut function_bodies = Arena::new();
                function_bodies.resize_with(function_count, || None);
                function_bodies
            },
            bodies: Arena::new(),
        }
    }

    pub fn status(&self) -> TargetBodiesStatus {
        self.status
    }

    pub fn body_for_function(&self, function: FunctionId) -> Option<BodyId> {
        self.function_bodies.get(function).copied().flatten()
    }

    pub fn body(&self, body: BodyId) -> Option<&BodyData> {
        self.bodies.get(body)
    }

    pub fn bodies(&self) -> &[BodyData] {
        self.bodies.as_slice()
    }

    fn shrink_to_fit(&mut self) {
        self.function_bodies.shrink_to_fit();
        self.bodies.shrink_to_fit();
        for body in self.bodies.iter_mut() {
            body.shrink_to_fit();
        }
    }
}

/// Whether one target's bodies were eagerly lowered.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub enum TargetBodiesStatus {
    #[display("built")]
    Built,
    #[display("skipped")]
    Skipped,
}

impl TargetBodies {
    pub(super) fn alloc_body(&mut self, data: BodyData) -> BodyId {
        self.bodies.alloc(data)
    }

    pub(super) fn set_function_body(&mut self, function: FunctionId, body: BodyId) {
        let slot = self
            .function_bodies
            .get_mut(function)
            .expect("function body slot should exist while building body IR");
        *slot = Some(body);
    }

    pub(super) fn bodies_mut(&mut self) -> &mut [BodyData] {
        self.bodies.as_mut_slice()
    }
}

/// Lowered body for one function.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct BodyData {
    pub(crate) owner: FunctionRef,
    pub(crate) owner_module: rg_def_map::ModuleRef,
    pub(crate) source: BodySource,
    pub(crate) param_scope: ScopeId,
    pub(crate) root_expr: ExprId,
    pub(crate) params: Vec<BindingId>,
    pub(crate) scopes: Arena<ScopeId, ScopeData>,
    pub(crate) local_items: Arena<BodyItemId, BodyItemData>,
    pub(crate) local_impls: Arena<BodyImplId, BodyImplData>,
    pub(crate) local_functions: Arena<BodyFunctionId, BodyFunctionData>,
    pub(crate) bindings: Arena<BindingId, BindingData>,
    pub(crate) pats: Arena<PatId, PatData>,
    pub(crate) statements: Arena<StmtId, StmtData>,
    pub(crate) exprs: Arena<ExprId, ExprData>,
}

impl BodyData {
    pub fn owner(&self) -> FunctionRef {
        self.owner
    }

    pub fn owner_module(&self) -> rg_def_map::ModuleRef {
        self.owner_module
    }

    pub fn source(&self) -> BodySource {
        self.source
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

    pub fn local_items(&self) -> &[BodyItemData] {
        self.local_items.as_slice()
    }

    pub fn local_impls(&self) -> &[BodyImplData] {
        self.local_impls.as_slice()
    }

    pub fn local_functions(&self) -> &[BodyFunctionData] {
        self.local_functions.as_slice()
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

    pub fn binding(&self, binding: BindingId) -> Option<&BindingData> {
        self.bindings.get(binding)
    }

    pub fn pat(&self, pat: PatId) -> Option<&PatData> {
        self.pats.get(pat)
    }

    pub fn scope(&self, scope: ScopeId) -> Option<&ScopeData> {
        self.scopes.get(scope)
    }

    pub fn local_item(&self, item: BodyItemId) -> Option<&BodyItemData> {
        self.local_items.get(item)
    }

    pub fn local_impl(&self, impl_id: BodyImplId) -> Option<&BodyImplData> {
        self.local_impls.get(impl_id)
    }

    pub fn local_function(&self, function: BodyFunctionId) -> Option<&BodyFunctionData> {
        self.local_functions.get(function)
    }

    pub fn statement(&self, statement: StmtId) -> Option<&StmtData> {
        self.statements.get(statement)
    }

    pub fn expr(&self, expr: ExprId) -> Option<&ExprData> {
        self.exprs.get(expr)
    }

    pub(super) fn new(
        owner: FunctionRef,
        owner_module: rg_def_map::ModuleRef,
        source: BodySource,
        param_scope: ScopeId,
        root_expr: ExprId,
        params: Vec<BindingId>,
        builder: BodyBuilder,
    ) -> Self {
        Self {
            owner,
            owner_module,
            source,
            param_scope,
            root_expr,
            params,
            scopes: builder.scopes,
            local_items: builder.local_items,
            local_impls: builder.local_impls,
            local_functions: builder.local_functions,
            bindings: builder.bindings,
            pats: builder.pats,
            statements: builder.statements,
            exprs: builder.exprs,
        }
    }

    fn shrink_to_fit(&mut self) {
        self.params.shrink_to_fit();
        self.scopes.shrink_to_fit();
        for scope in self.scopes.iter_mut() {
            scope.shrink_to_fit();
        }
        self.local_items.shrink_to_fit();
        for item in self.local_items.iter_mut() {
            item.shrink_to_fit();
        }
        self.local_impls.shrink_to_fit();
        for impl_data in self.local_impls.iter_mut() {
            impl_data.shrink_to_fit();
        }
        self.local_functions.shrink_to_fit();
        for function in self.local_functions.iter_mut() {
            function.shrink_to_fit();
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

    pub(crate) fn local_impl_mut(&mut self, impl_id: BodyImplId) -> Option<&mut BodyImplData> {
        self.local_impls.get_mut(impl_id)
    }

    pub(crate) fn inherent_functions_for_local_type(
        &self,
        body_ref: BodyRef,
        item_ref: BodyItemRef,
    ) -> Vec<BodyFunctionRef> {
        if item_ref.body != body_ref {
            return Vec::new();
        }

        let mut functions = Vec::new();
        for impl_data in self.local_impls.iter() {
            if impl_data.self_item != Some(item_ref) || impl_data.trait_ref.is_some() {
                continue;
            }

            for function in &impl_data.functions {
                functions.push(BodyFunctionRef {
                    body: body_ref,
                    function: *function,
                });
            }
        }

        functions
    }
}

/// Mutable store used while one body is being lowered.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct BodyBuilder {
    pub(super) scopes: Arena<ScopeId, ScopeData>,
    pub(super) local_items: Arena<BodyItemId, BodyItemData>,
    pub(super) local_impls: Arena<BodyImplId, BodyImplData>,
    pub(super) local_functions: Arena<BodyFunctionId, BodyFunctionData>,
    pub(super) bindings: Arena<BindingId, BindingData>,
    pub(super) pats: Arena<PatId, PatData>,
    pub(super) statements: Arena<StmtId, StmtData>,
    pub(super) exprs: Arena<ExprId, ExprData>,
}

impl BodyBuilder {
    pub(super) fn alloc_scope(&mut self, parent: Option<ScopeId>) -> ScopeId {
        self.scopes.alloc(ScopeData {
            parent,
            local_items: Vec::new(),
            local_impls: Vec::new(),
            bindings: Vec::new(),
        })
    }

    pub(super) fn alloc_local_item(&mut self, data: BodyItemData) -> BodyItemId {
        let scope = data.scope;
        let item = self.local_items.alloc(data);
        self.scopes
            .get_mut(scope)
            .expect("local item scope should exist while lowering body")
            .local_items
            .push(item);
        item
    }

    pub(super) fn alloc_local_impl(&mut self, data: BodyImplData) -> BodyImplId {
        let scope = data.scope;
        let impl_id = self.local_impls.alloc(data);
        self.scopes
            .get_mut(scope)
            .expect("local impl scope should exist while lowering body")
            .local_impls
            .push(impl_id);
        impl_id
    }

    pub(super) fn alloc_local_function(&mut self, data: BodyFunctionData) -> BodyFunctionId {
        self.local_functions.alloc(data)
    }

    pub(super) fn set_local_impl_functions(
        &mut self,
        impl_id: BodyImplId,
        functions: Vec<BodyFunctionId>,
    ) {
        let impl_data = self
            .local_impls
            .get_mut(impl_id)
            .expect("local impl should exist while assigning functions");
        impl_data.functions = functions;
    }

    pub(super) fn alloc_binding(&mut self, data: BindingData) -> BindingId {
        let scope = data.scope;
        let binding = self.bindings.alloc(data);
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

/// Source location attached to every body node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct BodySource {
    pub file_id: FileId,
    pub span: Span,
}

/// One lexical scope.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ScopeData {
    pub parent: Option<ScopeId>,
    pub local_items: Vec<BodyItemId>,
    pub local_impls: Vec<BodyImplId>,
    pub bindings: Vec<BindingId>,
}

impl ScopeData {
    fn shrink_to_fit(&mut self) {
        self.local_items.shrink_to_fit();
        self.local_impls.shrink_to_fit();
        self.bindings.shrink_to_fit();
    }
}
