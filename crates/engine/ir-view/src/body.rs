//! Generic body-local facts projected out of Body IR.
//!
//! Body IR owns lowered expression, scope, and local declaration storage. This view exposes the
//! parts that higher analysis features need without making them know the Body IR query vocabulary.

use std::collections::HashSet;

use rg_body_ir::{BindingKind, BodyItemKind, BodyItemOwner, BodyValueItemKind, BodyValueItemOwner};
use rg_ir_model::{
    BodyBindingRef, BodyFieldRef, BodyFunctionRef, BodyImplRef, BodyItemRef, BodyRef,
    BodyValueItemRef, ExprId, ModuleRef, ScopeId, TargetRef, identity::DeclarationRef,
};
use rg_parse::{FileId, Span, TextSpan};
use rg_ty::{IndexedTy, IndexedTyRepr};

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
        item: BodyItemRef,
        kind: BodyItemKind,
        label: String,
        scope_distance: usize,
        has_value_constructor: bool,
    },
    ValueItem {
        item: BodyValueItemRef,
        kind: BodyValueItemKind,
        label: String,
        scope_distance: usize,
    },
    Function {
        function: BodyFunctionRef,
        label: String,
        scope_distance: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferredBindingTy {
    file_id: FileId,
    span: Span,
    ty: IndexedTy,
}

impl InferredBindingTy {
    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn ty(&self) -> &IndexedTy {
        &self.ty
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
    analysis: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> BodyView<'a, 'db> {
    pub fn new(analysis: &'a IndexedViewDb<'db>) -> Self {
        Self { analysis }
    }

    pub fn owner_module(&self, body_ref: BodyRef) -> anyhow::Result<Option<ModuleRef>> {
        Ok(self
            .analysis
            .body_ir
            .body_data(body_ref)?
            .map(|body| body.owner_module()))
    }

    pub fn expr_ty(&self, body_ref: BodyRef, expr: ExprId) -> anyhow::Result<Option<IndexedTy>> {
        Ok(self
            .analysis
            .body_ir
            .body_data(body_ref)?
            .and_then(|body| body.expr(expr))
            .map(|expr| expr.ty.clone()))
    }

    pub fn binding_ty(&self, binding: BodyBindingRef) -> anyhow::Result<Option<IndexedTy>> {
        Ok(self
            .analysis
            .body_ir
            .body_data(binding.body)?
            .and_then(|body| body.binding(binding.binding))
            .map(|binding| binding.ty.clone()))
    }

    pub fn local_value_item_ty(&self, item: BodyValueItemRef) -> anyhow::Result<Option<IndexedTy>> {
        Ok(self
            .analysis
            .body_ir
            .body_data(item.body)?
            .and_then(|body| body.local_value_item(item.item))
            .and_then(|item| item.ty().cloned())
            .map(IndexedTyRepr::syntax))
    }

    pub fn receiver_ty(
        &self,
        body_ref: BodyRef,
        receiver: ExprId,
    ) -> anyhow::Result<Option<IndexedTy>> {
        self.expr_ty(body_ref, receiver)
    }

    pub fn fields_for_local_type(&self, item: BodyItemRef) -> anyhow::Result<Vec<BodyFieldRef>> {
        Ok(self.analysis.body_ir.fields_for_local_type(item)?)
    }

    pub fn lexical_names(&self, scope: BodyNameScope) -> anyhow::Result<Vec<BodyLexicalName>> {
        let Some(body) = self.analysis.body_ir.body_data(scope.body)? else {
            return Ok(Vec::new());
        };
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

                for function_id in scope_data.local_functions.iter().rev().copied() {
                    let Some(function) = body.local_function(function_id) else {
                        continue;
                    };
                    if !seen_values.insert(function.name.to_string()) {
                        continue;
                    }
                    names.push(BodyLexicalName::Function {
                        function: BodyFunctionRef {
                            body: scope.body,
                            function: function_id,
                        },
                        label: function.name.to_string(),
                        scope_distance,
                    });
                }

                for item_id in scope_data.local_value_items.iter().rev().copied() {
                    let Some(item) = body.local_value_item(item_id) else {
                        continue;
                    };
                    if !seen_values.insert(item.name.to_string()) {
                        continue;
                    }
                    names.push(BodyLexicalName::ValueItem {
                        item: BodyValueItemRef {
                            body: scope.body,
                            item: item_id,
                        },
                        kind: item.kind,
                        label: item.name.to_string(),
                        scope_distance,
                    });
                }
            }

            for item_id in scope_data.local_items.iter().rev().copied() {
                let Some(item) = body.local_item(item_id) else {
                    continue;
                };

                match scope.namespace {
                    BodyNameNamespace::Values => {
                        if !item.has_value_constructor()
                            || !seen_values.insert(item.name.to_string())
                        {
                            continue;
                        }
                    }
                    BodyNameNamespace::Types => {
                        if !seen_types.insert(item.name.to_string()) {
                            continue;
                        }
                    }
                }

                names.push(BodyLexicalName::TypeItem {
                    item: BodyItemRef {
                        body: scope.body,
                        item: item_id,
                    },
                    kind: item.kind,
                    label: item.name.to_string(),
                    scope_distance,
                    has_value_constructor: item.has_value_constructor(),
                });
            }

            scope_id = scope_data.parent;
            scope_distance += 1;
        }

        Ok(names)
    }

    pub fn inferred_binding_tys(
        &self,
        target: TargetRef,
        file_id: FileId,
        range: Option<TextSpan>,
    ) -> anyhow::Result<Vec<InferredBindingTy>> {
        let Some(target_bodies) = self.analysis.body_ir.target_bodies(target)? else {
            return Ok(Vec::new());
        };

        let mut bindings = Vec::new();
        for body in target_bodies.bodies() {
            for binding in body.bindings() {
                if binding.source.file_id != file_id {
                    continue;
                }
                if !matches!(binding.kind, BindingKind::Let) {
                    continue;
                }
                if binding.name.is_none() || binding.annotation.is_some() {
                    continue;
                }
                if matches!(binding.ty, IndexedTy::Unknown) {
                    continue;
                }
                if range.is_some_and(|range| !range.touches(binding.source.span.text.end)) {
                    continue;
                }

                bindings.push(InferredBindingTy {
                    file_id: binding.source.file_id,
                    span: binding.source.span,
                    ty: binding.ty.clone(),
                });
            }
        }

        Ok(bindings)
    }

    pub fn local_groups(
        &self,
        target: TargetRef,
        file_id: FileId,
    ) -> anyhow::Result<Vec<BodyLocalGroup>> {
        let Some(target_bodies) = self.analysis.body_ir.target_bodies(target)? else {
            return Ok(Vec::new());
        };

        let mut groups = Vec::new();
        for (body_idx, body) in target_bodies.bodies().iter().enumerate() {
            if body.source().file_id != file_id {
                continue;
            }

            groups.push(BodyLocalGroup {
                owner: DeclarationRef::semantic(body.owner().into()),
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
        let Some(body) = self.analysis.body_ir.body_data(body_ref)? else {
            return Ok(Vec::new());
        };
        let mut declarations = Vec::new();

        for (item_idx, item) in body.local_items().iter().enumerate() {
            if item.source.file_id == file_id && matches!(item.owner, BodyItemOwner::LocalScope(_))
            {
                declarations.push(DeclarationRef::body_item(BodyItemRef {
                    body: body_ref,
                    item: rg_ir_model::BodyItemId(item_idx),
                }));
            }
        }

        for (item_idx, item) in body.local_value_items().iter().enumerate() {
            if item.source.file_id == file_id
                && matches!(item.owner, BodyValueItemOwner::LocalScope(_))
            {
                declarations.push(DeclarationRef::body_value_item(BodyValueItemRef {
                    body: body_ref,
                    item: rg_ir_model::BodyValueItemId(item_idx),
                }));
            }
        }

        for (function_idx, function) in body.local_functions().iter().enumerate() {
            if function.source.file_id == file_id
                && matches!(function.owner, rg_body_ir::BodyFunctionOwner::LocalScope(_))
            {
                declarations.push(DeclarationRef::body_function(BodyFunctionRef {
                    body: body_ref,
                    function: rg_ir_model::BodyFunctionId(function_idx),
                }));
            }
        }

        for (impl_idx, impl_data) in body.local_impls().iter().enumerate() {
            if impl_data.source.file_id == file_id {
                declarations.push(DeclarationRef::body(
                    BodyImplRef {
                        body: body_ref,
                        impl_id: rg_ir_model::BodyImplId(impl_idx),
                    }
                    .into(),
                ));
            }
        }

        Ok(declarations)
    }
}
