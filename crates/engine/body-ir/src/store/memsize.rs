use crate::{
    BindingData, BindingId, BindingKind, BodyData, BodyFieldRef, BodyFunctionData, BodyFunctionId,
    BodyFunctionOwner, BodyFunctionRef, BodyGenericArg, BodyId, BodyImplData, BodyImplId,
    BodyIrBuildPolicy, BodyIrDb, BodyIrStats, BodyItemData, BodyItemId, BodyItemKind, BodyItemRef,
    BodyLocalNominalTy, BodyNominalTy, BodyPath, BodyRef, BodyResolution, BodySource, BodyTy,
    BodyTypePathResolution, ExprData, ExprId, ExprKind, LiteralKind, PackageBodies, PatData, PatId,
    PatKind, RecordExprField, RecordPatField, ResolvedFieldRef, ResolvedFunctionRef, ScopeData,
    ScopeId, StmtData, StmtKind, TargetBodies, TargetBodiesStatus,
    ir::expr::{ExprWrapperKind, LabelData, MatchArmData},
    ir::ids::StmtId,
};
use rg_memsize::{MemoryRecorder, MemorySize};

rg_memsize::impl_memory_size_leaf!(
    crate::BodyIrPackageScope,
    TargetBodiesStatus,
    ExprWrapperKind,
    LiteralKind,
    BodyItemKind,
    BindingKind,
    BodyId,
    BodyItemId,
    BodyImplId,
    BodyFunctionId,
    ExprId,
    PatId,
    StmtId,
    BindingId,
    ScopeId,
);

rg_memsize::impl_memory_size_children! {
    BodyIrBuildPolicy => package_scope;
    PackageBodies => targets;
    TargetBodies => status, function_bodies, bodies;
    BodyData => owner, owner_module, source, param_scope, root_expr, params, scopes,
        local_items, local_impls, local_functions, bindings, pats, statements, exprs;
    BodySource => file_id, span;
    ScopeData => parent, local_items, local_impls, bindings;
    ExprData => source, scope, visible_bindings, kind, resolution, ty;
    MatchArmData => pat, scope, expr;
    LabelData => name, span;
    RecordExprField => key, key_span, source_span, value;
    BodyPath => source_span, path, segment_spans;
    BodyLocalNominalTy => item, args;
    BodyNominalTy => def, args;
    BodyItemData => source, name_source, scope, kind, name, docs, generics, fields;
    BodyImplData => source, scope, generics, trait_ref, self_ty, self_item, functions;
    BodyFunctionData => source, name_source, owner, name, docs, declaration;
    PatData => source, kind;
    RecordPatField => key, key_span, source_span, pat;
    BindingData => source, scope, kind, name, annotation, ty;
    StmtData => source, kind;
    BodyRef => target, body;
    BodyItemRef => body, item;
    BodyFieldRef => item, index;
    BodyFunctionRef => body, function;
    BodyIrStats => target_count, built_target_count, skipped_target_count, body_count,
        scope_count, local_item_count, local_impl_count, local_function_count, binding_count,
        statement_count, expression_count;
}

impl MemorySize for BodyIrDb {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("packages", |recorder| {
            self.record_packages_memory_children(recorder);
        });
    }
}

impl MemorySize for ExprKind {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Block {
                label,
                scope,
                statements,
                tail,
            } => {
                recorder.scope("label", |recorder| label.record_memory_children(recorder));
                recorder.scope("scope", |recorder| scope.record_memory_children(recorder));
                recorder.scope("statements", |recorder| {
                    statements.record_memory_children(recorder);
                });
                recorder.scope("tail", |recorder| tail.record_memory_children(recorder));
            }
            Self::Path { path } => path.record_memory_children(recorder),
            Self::Call { callee, args } => {
                recorder.scope("callee", |recorder| callee.record_memory_children(recorder));
                recorder.scope("args", |recorder| args.record_memory_children(recorder));
            }
            Self::Match { scrutinee, arms } => {
                recorder.scope("scrutinee", |recorder| {
                    scrutinee.record_memory_children(recorder);
                });
                recorder.scope("arms", |recorder| arms.record_memory_children(recorder));
            }
            Self::If {
                condition,
                then_branch,
                else_branch,
            } => {
                recorder.scope("condition", |recorder| {
                    condition.record_memory_children(recorder);
                });
                recorder.scope("then_branch", |recorder| {
                    then_branch.record_memory_children(recorder);
                });
                recorder.scope("else_branch", |recorder| {
                    else_branch.record_memory_children(recorder);
                });
            }
            Self::Let {
                scope,
                pat,
                bindings,
                initializer,
            } => {
                recorder.scope("scope", |recorder| scope.record_memory_children(recorder));
                recorder.scope("pat", |recorder| pat.record_memory_children(recorder));
                recorder.scope("bindings", |recorder| {
                    bindings.record_memory_children(recorder);
                });
                recorder.scope("initializer", |recorder| {
                    initializer.record_memory_children(recorder);
                });
            }
            Self::Loop { label, body } => {
                recorder.scope("label", |recorder| label.record_memory_children(recorder));
                recorder.scope("body", |recorder| body.record_memory_children(recorder));
            }
            Self::While {
                label,
                condition,
                body,
            } => {
                recorder.scope("label", |recorder| label.record_memory_children(recorder));
                recorder.scope("condition", |recorder| {
                    condition.record_memory_children(recorder);
                });
                recorder.scope("body", |recorder| body.record_memory_children(recorder));
            }
            Self::For {
                label,
                scope,
                pat,
                bindings,
                iterable,
                body,
            } => {
                recorder.scope("label", |recorder| label.record_memory_children(recorder));
                recorder.scope("scope", |recorder| scope.record_memory_children(recorder));
                recorder.scope("pat", |recorder| pat.record_memory_children(recorder));
                recorder.scope("bindings", |recorder| {
                    bindings.record_memory_children(recorder);
                });
                recorder.scope("iterable", |recorder| {
                    iterable.record_memory_children(recorder);
                });
                recorder.scope("body", |recorder| body.record_memory_children(recorder));
            }
            Self::Break { label, value } => {
                recorder.scope("label", |recorder| label.record_memory_children(recorder));
                recorder.scope("value", |recorder| value.record_memory_children(recorder));
            }
            Self::Continue { label } => {
                recorder.scope("label", |recorder| label.record_memory_children(recorder));
            }
            Self::MethodCall {
                receiver,
                dot_span,
                method_name,
                method_name_span,
                args,
            } => {
                recorder.scope("receiver", |recorder| {
                    receiver.record_memory_children(recorder);
                });
                recorder.scope("dot_span", |recorder| {
                    dot_span.record_memory_children(recorder)
                });
                recorder.scope("method_name", |recorder| {
                    method_name.record_memory_children(recorder);
                });
                recorder.scope("method_name_span", |recorder| {
                    method_name_span.record_memory_children(recorder);
                });
                recorder.scope("args", |recorder| args.record_memory_children(recorder));
            }
            Self::Field {
                base,
                dot_span,
                field,
                field_span,
            } => {
                recorder.scope("base", |recorder| base.record_memory_children(recorder));
                recorder.scope("dot_span", |recorder| {
                    dot_span.record_memory_children(recorder)
                });
                recorder.scope("field", |recorder| field.record_memory_children(recorder));
                recorder.scope("field_span", |recorder| {
                    field_span.record_memory_children(recorder);
                });
            }
            Self::Record {
                path,
                field_list_span,
                fields,
                spread,
            } => {
                recorder.scope("path", |recorder| path.record_memory_children(recorder));
                recorder.scope("field_list_span", |recorder| {
                    field_list_span.record_memory_children(recorder);
                });
                recorder.scope("fields", |recorder| fields.record_memory_children(recorder));
                recorder.scope("spread", |recorder| spread.record_memory_children(recorder));
            }
            Self::Wrapper { kind, inner } => {
                recorder.scope("kind", |recorder| kind.record_memory_children(recorder));
                recorder.scope("inner", |recorder| inner.record_memory_children(recorder));
            }
            Self::Literal { kind } => kind.record_memory_children(recorder),
            Self::Unknown { children } => children.record_memory_children(recorder),
        }
    }
}

impl MemorySize for BodyResolution {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Local(binding) => binding.record_memory_children(recorder),
            Self::LocalItem(item) => item.record_memory_children(recorder),
            Self::Item(items) => items.record_memory_children(recorder),
            Self::Field(fields) => fields.record_memory_children(recorder),
            Self::Function(functions) | Self::Method(functions) => {
                functions.record_memory_children(recorder);
            }
            Self::EnumVariant(variants) => variants.record_memory_children(recorder),
            Self::Unknown => {}
        }
    }
}

impl MemorySize for ResolvedFieldRef {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Semantic(field) => field.record_memory_children(recorder),
            Self::BodyLocal(field) => field.record_memory_children(recorder),
        }
    }
}

impl MemorySize for ResolvedFunctionRef {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Semantic(function) => function.record_memory_children(recorder),
            Self::BodyLocal(function) => function.record_memory_children(recorder),
        }
    }
}

impl MemorySize for BodyTypePathResolution {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::BodyLocal(item) => item.record_memory_children(recorder),
            Self::SelfType(types) | Self::TypeDefs(types) => types.record_memory_children(recorder),
            Self::Traits(traits) => traits.record_memory_children(recorder),
            Self::Unknown => {}
        }
    }
}

impl MemorySize for BodyTy {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Unit | Self::Never | Self::Unknown => {}
            Self::Syntax(ty) => ty.record_memory_children(recorder),
            Self::Reference(inner) => inner.record_memory_children(recorder),
            Self::LocalNominal(types) => types.record_memory_children(recorder),
            Self::Nominal(types) | Self::SelfTy(types) => types.record_memory_children(recorder),
        }
    }
}

impl MemorySize for BodyGenericArg {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Type(ty) => ty.record_memory_children(recorder),
            Self::Lifetime(text) | Self::Const(text) | Self::Unsupported(text) => {
                text.record_memory_children(recorder);
            }
            Self::AssocType { name, ty } => {
                recorder.scope("name", |recorder| name.record_memory_children(recorder));
                recorder.scope("ty", |recorder| ty.record_memory_children(recorder));
            }
        }
    }
}

impl MemorySize for BodyFunctionOwner {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::LocalImpl(impl_id) => impl_id.record_memory_children(recorder),
        }
    }
}

impl MemorySize for PatKind {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Binding { binding, subpat } => {
                recorder.scope("binding", |recorder| {
                    binding.record_memory_children(recorder)
                });
                recorder.scope("subpat", |recorder| subpat.record_memory_children(recorder));
            }
            Self::Tuple { fields } | Self::Or { pats: fields } | Self::Slice { fields } => {
                fields.record_memory_children(recorder);
            }
            Self::TupleStruct { path, fields } => {
                recorder.scope("path", |recorder| path.record_memory_children(recorder));
                recorder.scope("fields", |recorder| fields.record_memory_children(recorder));
            }
            Self::Record {
                path,
                field_list_span,
                fields,
            } => {
                recorder.scope("path", |recorder| path.record_memory_children(recorder));
                recorder.scope("field_list_span", |recorder| {
                    field_list_span.record_memory_children(recorder);
                });
                recorder.scope("fields", |recorder| fields.record_memory_children(recorder));
            }
            Self::Ref { pat } | Self::Box { pat } => pat.record_memory_children(recorder),
            Self::Path { path } => path.record_memory_children(recorder),
            Self::Wildcard | Self::Unsupported => {}
        }
    }
}

impl MemorySize for StmtKind {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Let {
                scope,
                pat,
                bindings,
                annotation,
                initializer,
            } => {
                recorder.scope("scope", |recorder| scope.record_memory_children(recorder));
                recorder.scope("pat", |recorder| pat.record_memory_children(recorder));
                recorder.scope("bindings", |recorder| {
                    bindings.record_memory_children(recorder);
                });
                recorder.scope("annotation", |recorder| {
                    annotation.record_memory_children(recorder);
                });
                recorder.scope("initializer", |recorder| {
                    initializer.record_memory_children(recorder);
                });
            }
            Self::Expr {
                expr,
                has_semicolon,
            } => {
                recorder.scope("expr", |recorder| expr.record_memory_children(recorder));
                recorder.scope("has_semicolon", |recorder| {
                    has_semicolon.record_memory_children(recorder);
                });
            }
            Self::Item { item } => item.record_memory_children(recorder),
            Self::Impl { impl_id } => impl_id.record_memory_children(recorder),
            Self::ItemIgnored => {}
        }
    }
}
