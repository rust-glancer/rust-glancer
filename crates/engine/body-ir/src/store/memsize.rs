use crate::{
    BindingData, BindingId, BindingKind, BodyData, BodyEnumVariantRef, BodyFieldRef, BodyFloatTy,
    BodyFunctionData, BodyFunctionId, BodyFunctionOwner, BodyFunctionRef, BodyGenericArg, BodyId,
    BodyImplData, BodyImplId, BodyIrBuildPolicy, BodyIrDb, BodyIrStats, BodyItemData, BodyItemId,
    BodyItemKind, BodyItemRef, BodyLocalNominalTy, BodyNominalTy, BodyPath, BodyPrimitiveTy,
    BodyRef, BodyRefMutability, BodyResolution, BodySignedIntTy, BodySource, BodyTy,
    BodyTypePathResolution, BodyUnsignedIntTy, BodyValueItemData, BodyValueItemDeclaration,
    BodyValueItemId, BodyValueItemKind, BodyValueItemOwner, BodyValueItemRef, ClosureCapture,
    ClosureKind, ClosureParamData, ExprBlockKind, ExprData, ExprId, ExprKind, LiteralKind,
    PackageBodies, PatBindingMode, PatData, PatId, PatKind, PatMutability, PatRangeKind,
    RecordExprField, RecordExprSpread, RecordPatField, ResolvedEnumVariantRef, ResolvedFieldRef,
    ResolvedFunctionRef, ScopeData, ScopeId, StmtData, StmtKind, TargetBodies, TargetBodiesStatus,
    ir::expr::{ExprWrapperKind, LabelData, MatchArmData},
    ir::ids::StmtId,
    ir::item::{BodyItemDeclaration, BodyItemOwner},
    ir::path::{BodyPathSegment, BodyPathSegmentArgs, BodyPathSegmentKind},
};
use rg_memsize::{MemoryRecorder, MemorySize};

rg_memsize::impl_memory_size_leaf!(
    crate::BodyIrPackageScope,
    TargetBodiesStatus,
    ClosureCapture,
    ClosureKind,
    crate::ExprAssignOp,
    crate::ExprBinaryOp,
    crate::ExprRangeKind,
    crate::ExprUnaryOp,
    ExprWrapperKind,
    LiteralKind,
    PatBindingMode,
    PatMutability,
    PatRangeKind,
    BodyItemKind,
    BodyValueItemKind,
    BodyRefMutability,
    BodyItemOwner,
    BodyValueItemOwner,
    BindingKind,
    BodyFloatTy,
    BodyId,
    BodyItemId,
    BodyPrimitiveTy,
    BodySignedIntTy,
    BodyUnsignedIntTy,
    BodyValueItemId,
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
        local_items, local_value_items, local_impls, local_functions, bindings, pats, statements,
        exprs;
    BodySource => file_id, span;
    ScopeData => parent, local_items, local_value_items, local_functions, local_impls, bindings;
    ExprData => source, scope, visible_bindings, kind, resolution, ty;
    MatchArmData => pat, scope, guard, expr;
    ClosureParamData => source, pat, bindings, annotation;
    LabelData => name, span;
    RecordExprField => key, key_span, source_span, value;
    RecordExprSpread => source_span, expr;
    BodyPath => source_span, absolute, segments;
    BodyPathSegment => kind, span, args;
    BodyLocalNominalTy => item, args;
    BodyNominalTy => def, args;
    BodyItemData => source, name_source, scope, owner, kind, name, docs, declaration;
    BodyValueItemData => source, name_source, scope, owner, kind, name, docs, declaration;
    BodyImplData => source, scope, generics, trait_ref, self_ty, self_item, functions, consts,
        types;
    BodyFunctionData => source, name_source, owner, name, docs, declaration;
    PatData => source, kind;
    RecordPatField => key, key_span, source_span, pat;
    BindingData => source, scope, kind, name, annotation, ty;
    StmtData => source, kind;
    BodyRef => target, body;
    BodyItemRef => body, item;
    BodyValueItemRef => body, item;
    BodyFieldRef => item, index;
    BodyEnumVariantRef => item, index;
    BodyFunctionRef => body, function;
    BodyIrStats => target_count, built_target_count, skipped_target_count, body_count,
        scope_count, local_item_count, local_value_item_count, local_impl_count,
        local_function_count, binding_count, statement_count, expression_count;
}

impl MemorySize for ExprBlockKind {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Try { result_ty, .. } => {
                recorder.scope("result_ty", |recorder| {
                    result_ty.record_memory_children(recorder);
                });
            }
            Self::Plain
            | Self::Unsafe
            | Self::Const
            | Self::Async { .. }
            | Self::Gen { .. }
            | Self::AsyncGen { .. } => {}
        }
    }
}

impl MemorySize for BodyPathSegmentKind {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Name(name) => name.record_memory_children(recorder),
            Self::TypeAnchor { ty, trait_ref } => {
                recorder.scope("ty", |recorder| ty.record_memory_children(recorder));
                recorder.scope("trait_ref", |recorder| {
                    trait_ref.record_memory_children(recorder);
                });
            }
            Self::SelfType | Self::SelfKw | Self::SuperKw | Self::CrateKw => {}
        }
    }
}

impl MemorySize for BodyPathSegmentArgs {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Angle { colon_colon, args } => {
                recorder.scope("colon_colon", |recorder| {
                    colon_colon.record_memory_children(recorder);
                });
                recorder.scope("args", |recorder| args.record_memory_children(recorder));
            }
            Self::Parenthesized(text) => text.record_memory_children(recorder),
        }
    }
}

impl MemorySize for BodyIrDb {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("packages", |recorder| {
            self.record_packages_memory_children(recorder);
        });
    }
}

impl MemorySize for BodyItemDeclaration {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Struct(item) => item.record_memory_children(recorder),
            Self::Enum(item) => item.record_memory_children(recorder),
            Self::Union(item) => item.record_memory_children(recorder),
            Self::TypeAlias(item) => item.record_memory_children(recorder),
            Self::Trait(item) => item.record_memory_children(recorder),
        }
    }
}

impl MemorySize for BodyValueItemDeclaration {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Const(item) => item.record_memory_children(recorder),
            Self::Static(item) => item.record_memory_children(recorder),
        }
    }
}

impl MemorySize for ExprKind {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Block {
                kind,
                label,
                scope,
                statements,
                tail,
            } => {
                recorder.scope("kind", |recorder| kind.record_memory_children(recorder));
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
            Self::Tuple { fields } => fields.record_memory_children(recorder),
            Self::Array { elements } => elements.record_memory_children(recorder),
            Self::RepeatArray {
                initializer,
                repeat,
            } => {
                recorder.scope("initializer", |recorder| {
                    initializer.record_memory_children(recorder);
                });
                recorder.scope("repeat", |recorder| repeat.record_memory_children(recorder));
            }
            Self::Index { base, index } => {
                recorder.scope("base", |recorder| base.record_memory_children(recorder));
                recorder.scope("index", |recorder| index.record_memory_children(recorder));
            }
            Self::Range { start, end, kind } => {
                recorder.scope("start", |recorder| start.record_memory_children(recorder));
                recorder.scope("end", |recorder| end.record_memory_children(recorder));
                recorder.scope("kind", |recorder| kind.record_memory_children(recorder));
            }
            Self::Cast { expr, ty } => {
                recorder.scope("expr", |recorder| expr.record_memory_children(recorder));
                recorder.scope("ty", |recorder| ty.record_memory_children(recorder));
            }
            Self::Unary { op, expr } => {
                recorder.scope("op", |recorder| op.record_memory_children(recorder));
                recorder.scope("expr", |recorder| expr.record_memory_children(recorder));
            }
            Self::Binary { lhs, op, rhs } => {
                recorder.scope("lhs", |recorder| lhs.record_memory_children(recorder));
                recorder.scope("op", |recorder| op.record_memory_children(recorder));
                recorder.scope("rhs", |recorder| rhs.record_memory_children(recorder));
            }
            Self::Assign { target, op, value } => {
                recorder.scope("target", |recorder| {
                    target.record_memory_children(recorder);
                });
                recorder.scope("op", |recorder| op.record_memory_children(recorder));
                recorder.scope("value", |recorder| value.record_memory_children(recorder));
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
            Self::Closure {
                scope,
                capture,
                kind,
                params,
                ret_ty,
                body,
            } => {
                recorder.scope("scope", |recorder| scope.record_memory_children(recorder));
                recorder.scope("capture", |recorder| {
                    capture.record_memory_children(recorder);
                });
                recorder.scope("kind", |recorder| kind.record_memory_children(recorder));
                recorder.scope("params", |recorder| {
                    params.record_memory_children(recorder);
                });
                recorder.scope("ret_ty", |recorder| ret_ty.record_memory_children(recorder));
                recorder.scope("body", |recorder| body.record_memory_children(recorder));
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
            Self::Underscore => {}
            Self::Yield { value } | Self::Yeet { value } | Self::Become { value } => {
                value.record_memory_children(recorder);
            }
            Self::Unknown { children } => children.record_memory_children(recorder),
        }
    }
}

impl MemorySize for BodyResolution {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Local(binding) => binding.record_memory_children(recorder),
            Self::LocalItem(item) => item.record_memory_children(recorder),
            Self::LocalValueItem(item) => item.record_memory_children(recorder),
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

impl MemorySize for ResolvedEnumVariantRef {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Semantic(variant) => variant.record_memory_children(recorder),
            Self::BodyLocal(variant) => variant.record_memory_children(recorder),
        }
    }
}

impl MemorySize for BodyTypePathResolution {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::BodyLocal(item) => item.record_memory_children(recorder),
            Self::SelfType(types) | Self::TypeDefs(types) => types.record_memory_children(recorder),
            Self::Traits(traits) => traits.record_memory_children(recorder),
            Self::Primitive(_) | Self::Unknown => {}
        }
    }
}

impl MemorySize for BodyTy {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Unit | Self::Never | Self::Primitive(_) | Self::Unknown => {}
            Self::Syntax(ty) => ty.record_memory_children(recorder),
            Self::Reference { inner, .. } => inner.record_memory_children(recorder),
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
            Self::LocalScope(scope) => scope.record_memory_children(recorder),
            Self::LocalImpl(impl_id) => impl_id.record_memory_children(recorder),
        }
    }
}

impl MemorySize for PatKind {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Binding {
                mode,
                binding,
                subpat,
                path,
            } => {
                recorder.scope("mode", |recorder| mode.record_memory_children(recorder));
                recorder.scope("binding", |recorder| {
                    binding.record_memory_children(recorder)
                });
                recorder.scope("subpat", |recorder| subpat.record_memory_children(recorder));
                recorder.scope("path", |recorder| path.record_memory_children(recorder));
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
                rest,
            } => {
                recorder.scope("path", |recorder| path.record_memory_children(recorder));
                recorder.scope("field_list_span", |recorder| {
                    field_list_span.record_memory_children(recorder);
                });
                recorder.scope("fields", |recorder| fields.record_memory_children(recorder));
                recorder.scope("rest", |recorder| rest.record_memory_children(recorder));
            }
            Self::Ref { mutability, pat } => {
                recorder.scope("mutability", |recorder| {
                    mutability.record_memory_children(recorder);
                });
                recorder.scope("pat", |recorder| pat.record_memory_children(recorder));
            }
            Self::Box { pat } => pat.record_memory_children(recorder),
            Self::Path { path } => path.record_memory_children(recorder),
            Self::Literal { kind, negated } => {
                recorder.scope("kind", |recorder| kind.record_memory_children(recorder));
                recorder.scope("negated", |recorder| {
                    negated.record_memory_children(recorder)
                });
            }
            Self::Range { start, end, kind } => {
                recorder.scope("start", |recorder| start.record_memory_children(recorder));
                recorder.scope("end", |recorder| end.record_memory_children(recorder));
                recorder.scope("kind", |recorder| kind.record_memory_children(recorder));
            }
            Self::ConstBlock { expr } => {
                recorder.scope("expr", |recorder| expr.record_memory_children(recorder));
            }
            Self::Rest | Self::Wildcard | Self::Unsupported => {}
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
                else_branch,
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
                recorder.scope("else_branch", |recorder| {
                    else_branch.record_memory_children(recorder);
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
            Self::ValueItem { item } => item.record_memory_children(recorder),
            Self::Function { function } => function.record_memory_children(recorder),
            Self::Impl { impl_id } => impl_id.record_memory_children(recorder),
            Self::ItemIgnored => {}
        }
    }
}
