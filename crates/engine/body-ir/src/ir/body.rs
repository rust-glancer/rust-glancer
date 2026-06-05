use rg_arena::Arena;
use rg_ir_model::{
    BindingId, BodyId, BodyRef, ConstRef, DefMapRef, ExprId, FunctionRef, ModuleRef, PatId,
    ScopeId, StaticRef, StmtId, identity::DeclarationRef,
};
use rg_ir_storage::{DefMap, ItemStore};
use rg_item_tree::{ItemNode, ItemTreeId};
use rg_parse::{FileId, Span, TargetId};

use super::{
    body_map::BodySourceItems,
    expr::ExprData,
    pat::PatData,
    stmt::{BindingData, StmtData},
};

/// Coarse totals for reporting that the Body IR phase produced useful data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, rg_memsize::MemorySize)]
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
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Default,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
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
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct TargetBodies {
    pub(crate) status: TargetBodiesStatus,
    pub(crate) bodies: Arena<BodyId, BodyData>,
}

impl TargetBodies {
    pub(crate) fn new() -> Self {
        Self {
            status: TargetBodiesStatus::Built,
            bodies: Arena::new(),
        }
    }

    pub(crate) fn skipped() -> Self {
        Self {
            status: TargetBodiesStatus::Skipped,
            bodies: Arena::new(),
        }
    }

    pub fn status(&self) -> TargetBodiesStatus {
        self.status
    }

    pub fn body(&self, body: BodyId) -> Option<&BodyData> {
        self.bodies.get(body)
    }

    pub fn bodies(&self) -> &[BodyData] {
        self.bodies.as_slice()
    }

    pub(crate) fn alloc_body(&mut self, data: BodyData) {
        self.bodies.alloc(data);
    }

    pub(crate) fn bodies_mut(&mut self) -> &mut [BodyData] {
        self.bodies.as_mut_slice()
    }

    fn shrink_to_fit(&mut self) {
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
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub enum TargetBodiesStatus {
    #[display("built")]
    Built,
    #[display("skipped")]
    Skipped,
}

/// Semantic item that owns a lowered expression body.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub enum BodyOwner {
    /// Function body, such as `fn read() { value }`.
    Function(FunctionRef),
    /// Const initializer body, such as `const LIMIT: u8 = value;`.
    Const(ConstRef),
    /// Static initializer body, such as `static CURRENT: u8 = value;`.
    Static(StaticRef),
}

impl BodyOwner {
    /// Returns the function ref when this body is owned by a function declaration.
    pub fn function(self) -> Option<FunctionRef> {
        match self {
            Self::Function(function) => Some(function),
            Self::Const(_) | Self::Static(_) => None,
        }
    }

    /// Returns the declaration that should own facts derived from this body.
    pub fn declaration(self) -> DeclarationRef {
        match self {
            Self::Function(function) => DeclarationRef::from(function),
            Self::Const(const_ref) => DeclarationRef::from(const_ref),
            Self::Static(static_ref) => DeclarationRef::from(static_ref),
        }
    }
}

/// Lowered expression body for a function, const, or static initializer.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct BodyData {
    pub(crate) owner: BodyOwner,
    pub(crate) owner_module: ModuleRef,
    pub(crate) source: BodySource,
    pub(crate) source_items: BodySourceItems,
    pub(crate) body_def_map: Option<DefMap>,
    pub(crate) body_item_store: Option<ItemStore>,
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
    pub fn owner(&self) -> BodyOwner {
        self.owner
    }

    pub fn function_owner(&self) -> Option<FunctionRef> {
        self.owner.function()
    }

    pub fn owner_module(&self) -> ModuleRef {
        self.owner_module
    }

    pub fn source(&self) -> BodySource {
        self.source
    }

    pub fn source_items(&self) -> &BodySourceItems {
        &self.source_items
    }

    pub fn body_def_map(&self) -> Option<&DefMap> {
        self.body_def_map.as_ref()
    }

    pub fn body_item_store(&self) -> Option<&ItemStore> {
        self.body_item_store.as_ref()
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

    pub fn binding(&self, binding: BindingId) -> Option<&BindingData> {
        self.bindings.get(binding)
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

    pub(crate) fn new(
        owner: BodyOwner,
        owner_module: ModuleRef,
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
            source_items: builder.source_items,
            body_def_map: None,
            body_item_store: None,
            param_scope,
            root_expr,
            params,
            scopes: builder.scopes,
            bindings: builder.bindings,
            pats: builder.pats,
            statements: builder.statements,
            exprs: builder.exprs,
        }
    }

    fn shrink_to_fit(&mut self) {
        self.params.shrink_to_fit();
        self.source_items.shrink_to_fit();
        if let Some(def_map) = &mut self.body_def_map {
            def_map.shrink_to_fit();
        }
        if let Some(item_store) = &mut self.body_item_store {
            item_store.shrink_to_fit();
        }
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

/// Mutable store used while one body is being lowered.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct BodyBuilder {
    pub(crate) source_items: BodySourceItems,
    pub(crate) scopes: Arena<ScopeId, ScopeData>,
    pub(crate) bindings: Arena<BindingId, BindingData>,
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
        let scope = data.scope;
        let binding = self.bindings.alloc(data);
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

/// Source location attached to every body node.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct BodySource {
    pub file_id: FileId,
    pub span: Span,
}

/// One lexical scope.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
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
