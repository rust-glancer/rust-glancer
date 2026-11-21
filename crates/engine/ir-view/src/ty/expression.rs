//! Types of values and the types expected where those values are used.
//!
//! In `let user: User = cached`, the declared type of `user` describes what belongs
//! on the right. The binding type of `cached` describes what it can supply.

use rg_body_ir::{ExprKind, StmtKind};
use rg_ir_model::{
    BodyRef, DefMapRef, ExprId, FileId, FunctionRef, ImplRef, ItemStoreId, Path, SemanticItemRef,
    Span,
    identity::{DeclarationRef, LexicalScopeRef},
};
use rg_semantic_ir::{GenericsQuery, ItemStoreQuery, TypePathResolution};
use rg_std::ExpectedUnique;
use rg_ty::{
    ClosureTy, FnDefRef, Substitution, Ty,
    signature::{CallableSignature, SemanticSignatureQuery},
};

use super::{
    IndexedType, TyView,
    locals::{CallTarget, ResolvedCall},
};
use crate::{body::BodyResolutionView, member::MemberView, source::IndexedAssociatedPathQualifier};

impl TyView<'_, '_> {
    /// Look up the declared type of a value, including constants and statics behind a re-export.
    pub fn value_ty(&self, declaration: DeclarationRef) -> anyhow::Result<Option<IndexedType>> {
        let item = match declaration {
            DeclarationRef::Item(item) => Some(item),
            DeclarationRef::LocalDef(def) => {
                ItemStoreQuery::new(self.db).semantic_item_for_local_def(def)?
            }
            DeclarationRef::BodyBinding(binding) => return self.body_view().binding_ty(binding),
            _ => None,
        };
        let signatures = SemanticSignatureQuery::new(self.db, self.db);
        let ty = match item {
            Some(SemanticItemRef::Const(konst)) => signatures.const_ty(konst)?,
            Some(SemanticItemRef::Static(statik)) => signatures.static_ty(statik)?,
            _ => None,
        };
        Ok(ty.map(IndexedType::new))
    }

    /// Return the type produced by calling the function. Async functions produce a future.
    pub fn function_call_ty(&self, function: FunctionRef) -> anyhow::Result<Option<IndexedType>> {
        Ok(self
            .function_signature(function)?
            .map(|signature| IndexedType::new(signature.call_return)))
    }

    /// Read the arity of an associated function that constructs its own type.
    pub fn constructor_arity(&self, function: FunctionRef) -> anyhow::Result<Option<usize>> {
        if function.store.id == ItemStoreId::Scope {
            return Ok(None);
        }
        let members = MemberView::new(self.db);
        let Some(member) = members.function(function)? else {
            return Ok(None);
        };
        let signature = &member.data().signature;
        let Some(output) = signature.ret_ty() else {
            return Ok(None);
        };
        if member.has_self_receiver() || signature.qualifiers().is_async {
            return Ok(None);
        }

        let arity = member.parameters().len();
        // `-> Self` gives the answer directly. A written type or alias can also denote `Self`
        // if its lowered type and generic arguments agree with the impl header.
        if output.is_self_type() {
            return Ok(Some(arity));
        }
        let ItemStoreId::Impl(id) = function.store.id else {
            return Ok(None);
        };
        let Some(output) = self
            .function_call_ty(function)?
            .filter(IndexedType::is_known)
        else {
            return Ok(None);
        };
        let header = SemanticSignatureQuery::new(self.db, self.db)
            .impl_header(ImplRef::new(function.store.origin, id))?;
        Ok(header
            .is_some_and(|header| output.raw() == &header.self_ty)
            .then_some(arity))
    }

    /// Return the type expected by the function body, including for async functions.
    pub fn function_body_output(&self, body: BodyRef) -> anyhow::Result<Option<IndexedType>> {
        let Some(function) = self
            .db
            .body_ir
            .body(body)?
            .and_then(|body| body.function_owner())
        else {
            return Ok(None);
        };
        Ok(self
            .function_signature(function)?
            .map(|signature| IndexedType::new(signature.body_output().clone())))
    }

    /// Read the annotation directly. A destructuring pattern can bind several different types,
    /// and an empty initializer may have no lowered expression yet.
    pub fn let_annotation_ty(
        &self,
        body_ref: BodyRef,
        file: FileId,
        statement_start: u32,
    ) -> anyhow::Result<Option<IndexedType>> {
        let Some(body) = self.db.body_ir.body(body_ref)? else {
            return Ok(None);
        };
        let mut annotations = ExpectedUnique::new();
        for stmt in body.statements() {
            rg_std::check_cancel!(self.db, "find let annotation");
            let source = body.source_at(stmt.source);
            if source.is_written_in_file(file)
                && source.span.start == statement_start
                && let StmtKind::Let {
                    scope,
                    annotation: Some(annotation),
                    ..
                } = &stmt.kind
            {
                annotations.push((*scope, annotation));
            }
        }
        let Some((scope, annotation)) = annotations.into_option() else {
            return Ok(None);
        };
        Ok(BodyResolutionView::new(self.db)
            .type_ref_ty(body_ref, scope, annotation)?
            .map(IndexedType::new))
    }

    /// Project the type of a written expression such as an assignment's left-hand side.
    pub fn written_expr_ty(
        &self,
        body_ref: BodyRef,
        file: FileId,
        span: Span,
    ) -> anyhow::Result<Option<IndexedType>> {
        let Some(body) = self.db.body_ir.body(body_ref)? else {
            return Ok(None);
        };
        let mut types = ExpectedUnique::new();
        for (index, expr) in body.exprs().iter().enumerate() {
            rg_std::check_cancel!(self.db, "find written expression type");
            let source = body.source_at(expr.source);
            if source.is_written_in_file(file)
                && source.span == span
                && let Some(ty) = body.expr_ty(ExprId(index))
            {
                types.push(ty);
            }
        }
        Ok(types.into_option().cloned().map(IndexedType::new))
    }

    /// Read the parameter type independently of the argument the user may replace.
    ///
    /// For `pair<T>(first: T, second: T)`, completing the second argument in
    /// `pair(1_u8, old_value)` should use the `u8` from the first argument. If the
    /// signature needs inference, ask what the slot would expect with `old_value` omitted.
    pub fn call_parameter_ty(
        &self,
        call: &ResolvedCall<CallTarget>,
        argument_index: usize,
        active_argument_span: Option<Span>,
    ) -> anyhow::Result<Option<IndexedType>> {
        let expr = call.expr();
        let body_ref = expr.body_ir();
        let Some(body) = self.db.body_ir.body(body_ref)? else {
            return Ok(None);
        };
        let Some(data) = body.expr(expr.expr_id()) else {
            return Ok(None);
        };
        let args = match &data.kind {
            ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => args,
            _ => return Ok(None),
        };
        // A written slot can be empty, and parser recovery can shift the lowered
        // arguments. Use the span to make sure we found the expression to omit.
        let active_arg = active_argument_span.and_then(|span| {
            let &arg = args.get(argument_index)?;
            let expr = body.expr(arg)?;
            span.contains_span(body.span(expr.source))
                .then_some((arg, expr))
        });

        // An empty slot or unresolved path leaves us free to use the stored call
        // substitutions. For a filled slot, start from the declaration with its
        // generic arguments unknown. A fixed parameter like `bool` is already usable.
        let use_inferred_args = active_argument_span.is_none()
            || active_arg.is_some_and(|(arg, expr)| {
                matches!(expr.kind, ExprKind::Path { .. })
                    && body.expr_declarations(body_ref, arg).is_empty()
            });
        let infer_argument = || {
            if let Some((arg, _)) = active_arg {
                BodyResolutionView::new(self.db).call_argument_ty(body_ref, arg)
            } else {
                Ok(None)
            }
        };

        let index = argument_index + call.param_offset();
        let (def, inferred_args) = match call.target() {
            CallTarget::Function(function) => {
                let facts = body.call_facts(expr.expr_id());
                let function = facts.map_or(*function, |facts| facts.function());
                (function.into(), facts.map(|facts| facts.generic_args()))
            }
            CallTarget::Value(callable) => match callable.raw() {
                // The callable's inferred signature may also depend on this use.
                Ty::FnPointer { params, .. } | Ty::Closure(ClosureTy { params, .. }) => {
                    let ty = match params.get(index) {
                        Some(_) if !use_inferred_args => infer_argument()?,
                        param => param.cloned(),
                    };
                    return Ok(ty.map(IndexedType::new));
                }
                Ty::FnDef(item) => (item.def, Some(&item.args)),
                _ => return Ok(None),
            },
        };

        let owner = match def {
            FnDefRef::Function(function) => function.into(),
            FnDefRef::Constructor(constructor) => constructor.owner().into(),
        };
        let generics = GenericsQuery::new(self.db).generics(owner)?;
        let unknown_args;
        let args = if let Some(inferred) = inferred_args.filter(|_| use_inferred_args) {
            inferred
        } else {
            unknown_args = Substitution::new().args_for(&generics);
            &unknown_args
        };

        let ty = match def {
            FnDefRef::Function(function) => {
                self.function_signature(function)?.and_then(|signature| {
                    let param = signature.params.get(index)?;
                    Some(Substitution::from_args(&generics, args).apply(param))
                })
            }
            FnDefRef::Constructor(constructor) => SemanticSignatureQuery::new(self.db, self.db)
                .constructor(constructor, args)?
                .and_then(|signature| signature.params.into_iter().nth(index)),
        };
        let ty = if !use_inferred_args
            && ty
                .as_ref()
                .is_some_and(|ty| ty.has_unknown_type_or_const() || ty.has_projection())
        {
            infer_argument()?
        } else {
            ty
        };
        Ok(ty.map(IndexedType::new))
    }

    /// Read the type in a qualifier like `Event::<u8>::Received`, where the
    /// qualifier fixes the enum's generic arguments.
    pub fn nominal_qualifier_ty(
        &self,
        scope: LexicalScopeRef,
        qualifier: &IndexedAssociatedPathQualifier,
    ) -> anyhow::Result<Option<IndexedType>> {
        let IndexedAssociatedPathQualifier::Type(ty) = qualifier else {
            return Ok(None);
        };
        Ok(BodyResolutionView::new(self.db)
            .type_ref_ty(scope.body_ir(), scope.scope_id(), ty)?
            .filter(|ty| matches!(ty, Ty::Adt(_)))
            .map(IndexedType::new))
    }

    /// Check a constructor qualifier that completion will insert without generic arguments.
    pub fn bare_constructor_matches(
        &self,
        scope: LexicalScopeRef,
        path: &Path,
        expected: &IndexedType,
    ) -> anyhow::Result<bool> {
        let resolution = BodyResolutionView::new(self.db).type_path_resolution(
            scope.body_ir(),
            scope.scope_id(),
            path,
        )?;
        match resolution {
            Some(TypePathResolution::TypeDef(def) | TypePathResolution::SelfType(def)) => {
                Ok(expected.adt_def() == Some(def))
            }
            Some(TypePathResolution::TypeAlias(alias)) => {
                let owner = self.body_view().generic_owner(scope.body_ir())?;
                Ok(
                    SemanticSignatureQuery::new(self.db, self.db).type_alias_matches_adt(
                        alias,
                        expected.raw(),
                        owner,
                    )?,
                )
            }
            _ => Ok(false),
        }
    }

    fn function_signature(
        &self,
        function: FunctionRef,
    ) -> anyhow::Result<Option<CallableSignature>> {
        if let DefMapRef::Body(body) = function.store.origin {
            BodyResolutionView::new(self.db).function_signature(body, function)
        } else {
            Ok(SemanticSignatureQuery::new(self.db, self.db).function(function)?)
        }
    }
}
