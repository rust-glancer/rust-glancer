//! Generic body-local facts projected out of Body IR.
//!
//! Body IR owns lowered expression, scope, and local declaration storage. This view exposes the
//! parts that higher analysis features need without making them know the Body IR query vocabulary.

use std::collections::HashSet;

use rg_body_ir::BindingKind;
use rg_ir_model::{
    BindingId, BodyBindingRef, BodyRef, DefMapRef, ExprId, ExprKind, FunctionRef, ModuleId,
    ModuleRef, ScopeId, SemanticItemKind, SemanticItemRef, TargetRef, hir::source::ItemSourceKind,
    identity::DeclarationRef,
};
use rg_ir_storage::ItemStoreQuery;
use rg_parse::{FileId, Span, TextSpan};
use rg_ty::Ty;

use crate::IndexedViewDb;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyNameNamespace {
    Types,
    Values,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyNameScope {
    body: BodyRef,
    scope: ScopeId,
    namespace: BodyNameNamespace,
    visible_bindings: usize,
}

impl BodyNameScope {
    pub fn new(
        body: BodyRef,
        scope: ScopeId,
        namespace: BodyNameNamespace,
        visible_bindings: usize,
    ) -> Self {
        Self {
            body,
            scope,
            namespace,
            visible_bindings,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyLexicalName {
    Binding {
        binding: BodyBindingRef,
        label: String,
        scope_distance: usize,
    },
    TypeItem {
        item: SemanticItemRef,
        kind: SemanticItemKind,
        label: String,
        scope_distance: usize,
        has_value_constructor: bool,
    },
    ValueItem {
        item: SemanticItemRef,
        kind: SemanticItemKind,
        label: String,
        scope_distance: usize,
    },
    Function {
        function: rg_ir_model::FunctionRef,
        label: String,
        scope_distance: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferredBindingTy {
    file_id: FileId,
    span: Span,
    ty: Ty,
}

impl InferredBindingTy {
    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn ty(&self) -> &Ty {
        &self.ty
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedCallArg {
    span: Span,
}

impl ResolvedCallArg {
    pub fn span(&self) -> Span {
        self.span
    }
}

/// A call site whose arguments can be related back to one function signature.
///
/// This gives higher analysis features a stable way to talk about call-site arguments in
/// declaration terms, regardless of whether the source used a free call, an associated call, or a
/// receiver method call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFunctionCall {
    file_id: FileId,
    function: FunctionRef,
    param_offset: usize,
    args: Vec<ResolvedCallArg>,
}

impl ResolvedFunctionCall {
    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    pub fn function(&self) -> FunctionRef {
        self.function
    }

    pub fn param_offset(&self) -> usize {
        self.param_offset
    }

    pub fn args(&self) -> &[ResolvedCallArg] {
        &self.args
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyLocalGroup {
    owner: DeclarationRef,
    body: BodyRef,
}

impl BodyLocalGroup {
    pub fn owner(&self) -> DeclarationRef {
        self.owner
    }

    pub fn body(&self) -> BodyRef {
        self.body
    }
}

pub struct BodyView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> BodyView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub fn owner_module(&self, body_ref: BodyRef) -> anyhow::Result<Option<ModuleRef>> {
        Ok(self
            .db
            .body_ir
            .body_data(body_ref)?
            .map(|body| body.owner_module()))
    }

    pub fn lexical_scope_modules(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
    ) -> anyhow::Result<Vec<(ScopeId, ModuleRef)>> {
        let Some(body) = self.db.body_ir.body_data(body_ref)? else {
            return Ok(Vec::new());
        };
        let mut modules = Vec::new();
        let mut scope_id = Some(scope);

        while let Some(current_scope) = scope_id {
            let Some(scope_data) = body.scope(current_scope) else {
                break;
            };
            let module = ModuleRef {
                origin: DefMapRef::Body(body_ref),
                module: ModuleId(current_scope.0),
            };
            modules.push((current_scope, module));
            scope_id = scope_data.parent;
        }

        Ok(modules)
    }

    pub fn direct_item_names(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
    ) -> anyhow::Result<HashSet<String>> {
        let Some(body) = self.db.body_ir.body_data(body_ref)? else {
            return Ok(HashSet::new());
        };
        let Some(scope_data) = body.scope(scope) else {
            return Ok(HashSet::new());
        };

        let mut names = HashSet::new();
        for item_id in &scope_data.source_items {
            let Some(item) = body.source_item(*item_id) else {
                continue;
            };
            if let Some(name) = &item.name {
                names.insert(name.to_string());
            }
        }

        Ok(names)
    }

    pub fn expr_ty(&self, body_ref: BodyRef, expr: ExprId) -> anyhow::Result<Option<Ty>> {
        Ok(self
            .db
            .body_ir
            .body_data(body_ref)?
            .and_then(|body| body.expr_ty(expr).cloned()))
    }

    pub fn binding_ty(&self, binding: BodyBindingRef) -> anyhow::Result<Option<Ty>> {
        Ok(self
            .db
            .body_ir
            .body_data(binding.body)?
            .and_then(|body| body.binding_ty(binding.binding).cloned()))
    }

    pub fn lexical_names(&self, scope: BodyNameScope) -> anyhow::Result<Vec<BodyLexicalName>> {
        let Some(body) = self.db.body_ir.body_data(scope.body)? else {
            return Ok(Vec::new());
        };
        let body_item_store = self.db.body_ir.body_item_store(scope.body)?;
        let mut names = Vec::new();
        let mut seen_values = HashSet::<String>::new();
        let mut seen_types = HashSet::<String>::new();
        let mut scope_id = Some(scope.scope);
        let mut scope_distance = 0;

        // Lexical names are visible from the innermost scope outward. The first name wins in each
        // namespace, matching normal shadowing while keeping the result useful for ranking.
        while let Some(current_scope) = scope_id {
            let Some(scope_data) = body.scope(current_scope) else {
                break;
            };

            if matches!(scope.namespace, BodyNameNamespace::Values) {
                for binding_id in scope_data.bindings.iter().rev().copied() {
                    if binding_id.0 >= scope.visible_bindings {
                        continue;
                    }
                    let Some(binding) = body.binding(binding_id) else {
                        continue;
                    };
                    let Some(name) = binding.name.as_ref() else {
                        continue;
                    };
                    if !seen_values.insert(name.to_string()) {
                        continue;
                    }
                    names.push(BodyLexicalName::Binding {
                        binding: BodyBindingRef {
                            body: scope.body,
                            binding: binding_id,
                        },
                        label: name.to_string(),
                        scope_distance,
                    });
                }

                for item_id in scope_data.source_items.iter().rev().copied() {
                    let Some(view) = body_item_store.and_then(|items| {
                        items.semantic_items().find(|view| {
                            matches!(
                                view.source().kind,
                                ItemSourceKind::Body(source)
                                    if source.body == scope.body && source.item == item_id
                            )
                        })
                    }) else {
                        continue;
                    };
                    let Some(name) = view.name() else {
                        continue;
                    };

                    match view.item() {
                        SemanticItemRef::Function(function) => {
                            if !seen_values.insert(name.to_string()) {
                                continue;
                            }
                            names.push(BodyLexicalName::Function {
                                function,
                                label: name.to_string(),
                                scope_distance,
                            });
                        }
                        SemanticItemRef::Const(_) | SemanticItemRef::Static(_) => {
                            if !seen_values.insert(name.to_string()) {
                                continue;
                            }
                            names.push(BodyLexicalName::ValueItem {
                                item: view.item(),
                                kind: view.kind(),
                                label: name.to_string(),
                                scope_distance,
                            });
                        }
                        SemanticItemRef::TypeDef(ty) => {
                            let has_value_constructor =
                                ItemStoreQuery::new(self.db).type_def_has_value_constructor(ty)?;
                            if !has_value_constructor || !seen_values.insert(name.to_string()) {
                                continue;
                            }
                            names.push(BodyLexicalName::TypeItem {
                                item: view.item(),
                                kind: view.kind(),
                                label: name.to_string(),
                                scope_distance,
                                has_value_constructor,
                            });
                        }
                        SemanticItemRef::Trait(_)
                        | SemanticItemRef::Impl(_)
                        | SemanticItemRef::TypeAlias(_) => {}
                    }
                }
            }

            if matches!(scope.namespace, BodyNameNamespace::Types) {
                for item_id in scope_data.source_items.iter().rev().copied() {
                    let Some(view) = body_item_store.and_then(|items| {
                        items.semantic_items().find(|view| {
                            matches!(
                                view.source().kind,
                                ItemSourceKind::Body(source)
                                    if source.body == scope.body && source.item == item_id
                            )
                        })
                    }) else {
                        continue;
                    };
                    if !matches!(
                        view.item(),
                        SemanticItemRef::TypeDef(_)
                            | SemanticItemRef::Trait(_)
                            | SemanticItemRef::TypeAlias(_)
                    ) {
                        continue;
                    }
                    let Some(name) = view.name() else {
                        continue;
                    };
                    if !seen_types.insert(name.to_string()) {
                        continue;
                    }
                    let has_value_constructor = match view.item() {
                        SemanticItemRef::TypeDef(ty) => {
                            ItemStoreQuery::new(self.db).type_def_has_value_constructor(ty)?
                        }
                        _ => false,
                    };
                    names.push(BodyLexicalName::TypeItem {
                        item: view.item(),
                        kind: view.kind(),
                        label: name.to_string(),
                        scope_distance,
                        has_value_constructor,
                    });
                }
            }

            scope_id = scope_data.parent;
            scope_distance += 1;
        }

        Ok(names)
    }

    /// Returns let-like bindings whose type is already known from body facts.
    ///
    /// These are the local pattern bindings that can carry inferred type hints: ordinary `let`
    /// bindings, `let else` and match-pattern bindings, and `for` loop pattern bindings.
    pub fn inferred_binding_tys(
        &self,
        target: TargetRef,
        file_id: FileId,
        range: Option<TextSpan>,
    ) -> anyhow::Result<Vec<InferredBindingTy>> {
        let Some(target_bodies) = self.db.body_ir.target_bodies(target)? else {
            return Ok(Vec::new());
        };

        let mut bindings = Vec::new();
        for body in target_bodies.bodies() {
            for (binding_idx, binding) in body.bindings().iter().enumerate() {
                if binding.source.file_id != file_id {
                    continue;
                }
                if !matches!(binding.kind, BindingKind::Let) {
                    continue;
                }
                if binding.name.is_none() || binding.annotation.is_some() {
                    continue;
                }
                let ty = body
                    .binding_ty(BindingId(binding_idx))
                    .cloned()
                    .unwrap_or(Ty::Unknown);
                if matches!(ty, Ty::Unknown) {
                    continue;
                }
                if range.is_some_and(|range| !range.touches(binding.source.span.text.end)) {
                    continue;
                }

                bindings.push(InferredBindingTy {
                    file_id: binding.source.file_id,
                    span: binding.source.span,
                    ty,
                });
            }
        }

        Ok(bindings)
    }

    /// Returns call sites in one file that resolve to a single known function.
    ///
    /// This is the body-local view used by features that need to project declaration metadata, such
    /// as parameter names, onto concrete argument expressions without owning call resolution.
    pub fn resolved_function_calls(
        &self,
        target: TargetRef,
        file_id: FileId,
    ) -> anyhow::Result<Vec<ResolvedFunctionCall>> {
        let Some(target_bodies) = self.db.body_ir.target_bodies(target)? else {
            return Ok(Vec::new());
        };

        let mut calls = Vec::new();
        for (body_idx, body) in target_bodies.bodies().iter().enumerate() {
            let body_ref = BodyRef {
                target,
                body: rg_ir_model::BodyId(body_idx),
            };

            for (expr_idx, expr) in body.exprs().iter().enumerate() {
                if expr.source.file_id != file_id {
                    continue;
                }

                match &expr.kind {
                    ExprKind::Call { callee, args } => {
                        let Some(callee) = *callee else {
                            continue;
                        };
                        let Some(function) =
                            self.single_function(body.expr_declarations(body_ref, callee))?
                        else {
                            continue;
                        };
                        calls.push(ResolvedFunctionCall {
                            file_id: expr.source.file_id,
                            function,
                            param_offset: 0,
                            args: Self::resolved_call_args(body, args),
                        });
                    }
                    ExprKind::MethodCall { args, .. } => {
                        let Some(function) = self
                            .single_function(body.expr_declarations(body_ref, ExprId(expr_idx)))?
                        else {
                            continue;
                        };
                        calls.push(ResolvedFunctionCall {
                            file_id: expr.source.file_id,
                            function,
                            param_offset: 1,
                            args: Self::resolved_call_args(body, args),
                        });
                    }
                    _ => {}
                }
            }
        }

        Ok(calls)
    }

    pub fn local_groups(
        &self,
        target: TargetRef,
        file_id: FileId,
    ) -> anyhow::Result<Vec<BodyLocalGroup>> {
        let Some(target_bodies) = self.db.body_ir.target_bodies(target)? else {
            return Ok(Vec::new());
        };

        let mut groups = Vec::new();
        for (body_idx, body) in target_bodies.bodies().iter().enumerate() {
            if body.source().file_id != file_id {
                continue;
            }

            groups.push(BodyLocalGroup {
                owner: body.owner().declaration(),
                body: BodyRef {
                    target,
                    body: rg_ir_model::BodyId(body_idx),
                },
            });
        }

        Ok(groups)
    }

    pub fn local_scope_declarations(
        &self,
        body_ref: BodyRef,
        file_id: FileId,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let Some(body) = self.db.body_ir.body_data(body_ref)? else {
            return Ok(Vec::new());
        };
        let body_item_store = self.db.body_ir.body_item_store(body_ref)?;
        let mut declarations = Vec::new();

        for scope in body.scopes() {
            for item_id in &scope.source_items {
                let Some(view) = body_item_store.and_then(|items| {
                    items.semantic_items().find(|view| {
                        matches!(
                            view.source().kind,
                            ItemSourceKind::Body(source)
                                if source.body == body_ref && source.item == *item_id
                        )
                    })
                }) else {
                    continue;
                };
                if view.source().file_id == file_id {
                    declarations.push(DeclarationRef::from(view.item()));
                }
            }
        }

        Ok(declarations)
    }

    fn single_function(
        &self,
        declarations: Vec<DeclarationRef>,
    ) -> anyhow::Result<Option<FunctionRef>> {
        let mut functions = Vec::new();
        for declaration in declarations {
            match declaration {
                DeclarationRef::LocalDef(local_def) => {
                    let Some(SemanticItemRef::Function(function)) =
                        ItemStoreQuery::new(self.db).semantic_item_for_local_def(local_def)?
                    else {
                        continue;
                    };
                    functions.push(function);
                }
                DeclarationRef::Item(SemanticItemRef::Function(function)) => {
                    functions.push(function);
                }
                DeclarationRef::Module(_)
                | DeclarationRef::Item(
                    SemanticItemRef::TypeDef(_)
                    | SemanticItemRef::Trait(_)
                    | SemanticItemRef::Impl(_)
                    | SemanticItemRef::TypeAlias(_)
                    | SemanticItemRef::Const(_)
                    | SemanticItemRef::Static(_),
                )
                | DeclarationRef::Field(_)
                | DeclarationRef::EnumVariant(_)
                | DeclarationRef::BodyBinding(_) => {}
            }
        }

        let mut functions = functions.into_iter();
        let Some(function) = functions.next() else {
            return Ok(None);
        };
        Ok(functions.next().is_none().then_some(function))
    }

    fn resolved_call_args(
        body: &rg_body_ir::ResolvedBodyData,
        args: &[ExprId],
    ) -> Vec<ResolvedCallArg> {
        args.iter()
            .filter_map(|arg| {
                body.expr(*arg).map(|expr| ResolvedCallArg {
                    span: expr.source.span,
                })
            })
            .collect()
    }
}
