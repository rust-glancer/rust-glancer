//! Goto-implementation query flow.

use rg_body_ir::ExprKind;
use rg_ir_model::{
    FunctionRef, ImplRef, SemanticItemRef, TargetRef,
    identity::{DeclarationRef, ExprRef},
};
use rg_ir_storage::{ItemStoreQuery, TargetItemQuery};
use rg_parse::FileId;
use rg_ty::{ImplementationQuery, ItemPathQuery, Ty};

use super::target::NavigationTargetProjection;
use crate::{
    Analysis,
    model::{NavigationTarget, SymbolAt},
    source_symbol::SourceSymbolResolver,
};

/// Implements goto-implementation with the facts rust-glancer already collects.
///
/// The query deliberately returns concrete source declarations only: impl blocks for types/traits
/// and concrete methods for trait-method declarations or calls. It avoids inventing targets for
/// default trait items because those are declarations, not user-written implementations.
pub(crate) struct ImplementationResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> ImplementationResolver<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn goto_implementation(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        let Some(symbol) = self.0.symbol_at_for_query(target, file_id, offset)? else {
            return Ok(Vec::new());
        };

        if let SymbolAt::Expr { expr } = &symbol
            && let Some(declarations) = self.implementations_for_method_call_expr(*expr)?
        {
            return NavigationTargetProjection::new(self.0.view_db())
                .targets_for_declarations(declarations);
        }

        let mut declarations = Vec::new();
        let source_symbols = SourceSymbolResolver::new(self.0.view_db());
        for declaration in source_symbols.declarations_for_symbol(symbol.clone())? {
            Self::extend_unique_declarations(
                &mut declarations,
                self.implementations_for_declaration(target, declaration)?,
            );
        }

        if declarations.is_empty()
            && let Some(ty) = source_symbols.ty_for_symbol(symbol)?
        {
            Self::extend_unique_declarations(
                &mut declarations,
                self.implementations_for_ty(target, &ty)?,
            );
        }

        NavigationTargetProjection::new(self.0.view_db()).targets_for_declarations(declarations)
    }

    fn implementations_for_method_call_expr(
        &self,
        expr: ExprRef,
    ) -> anyhow::Result<Option<Vec<DeclarationRef>>> {
        let body_ref = expr.body_ir();
        let Some(body_data) = self.0.view_db().body_data(body_ref)? else {
            return Ok(None);
        };
        let Some(expr_data) = body_data.expr(expr.expr_id()) else {
            return Ok(None);
        };
        let ExprKind::MethodCall {
            receiver: Some(receiver),
            ..
        } = &expr_data.kind
        else {
            return Ok(None);
        };
        let receiver_ty = body_data.expr_ty(*receiver);
        let declarations = rg_ir_view::lookup::resolution::ResolutionView::new(self.0.view_db())
            .declarations_for_expr(expr)?;
        if declarations.is_empty() {
            return Ok(None);
        }

        let implementation_query = ImplementationQuery::new(
            ItemPathQuery::new(self.0.view_db(), self.0.view_db()),
            TargetItemQuery::new(self.0.view_db(), self.0.view_db(), body_ref.target),
        );
        let mut implementations = Vec::new();
        for declaration in declarations {
            let Some(function) = self.function_ref_for_declaration(declaration)? else {
                continue;
            };
            Self::extend_function_refs(
                &mut implementations,
                implementation_query.function_implementations(function, receiver_ty)?,
            );
        }
        Ok(Some(implementations))
    }

    fn implementations_for_declaration(
        &self,
        use_site: TargetRef,
        declaration: DeclarationRef,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let implementation_query = ImplementationQuery::new(
            ItemPathQuery::new(self.0.view_db(), self.0.view_db()),
            TargetItemQuery::new(self.0.view_db(), self.0.view_db(), use_site),
        );
        let mut implementations = Vec::new();

        match declaration {
            DeclarationRef::Item(item) => match item {
                SemanticItemRef::TypeDef(ty) => {
                    Self::extend_impl_refs(
                        &mut implementations,
                        implementation_query.impls_for_type_def(ty)?,
                    );
                }
                SemanticItemRef::Trait(trait_ref) => {
                    Self::extend_impl_refs(
                        &mut implementations,
                        implementation_query.impls_for_trait(trait_ref)?,
                    );
                }
                SemanticItemRef::Function(function) => {
                    Self::extend_function_refs(
                        &mut implementations,
                        implementation_query.function_implementations(function, None)?,
                    );
                }
                SemanticItemRef::Impl(_)
                | SemanticItemRef::TypeAlias(_)
                | SemanticItemRef::Const(_)
                | SemanticItemRef::Static(_) => {}
            },
            DeclarationRef::LocalDef(local_def) => {
                let Some(function) =
                    self.function_ref_for_declaration(DeclarationRef::local_def(local_def))?
                else {
                    return Ok(implementations);
                };
                Self::extend_function_refs(
                    &mut implementations,
                    implementation_query.function_implementations(function, None)?,
                );
            }
            DeclarationRef::BodyBinding(binding) => {
                let Some(body) = self.0.view_db().body_data(binding.body)? else {
                    return Ok(implementations);
                };
                let Some(binding_ty) = body.binding_ty(binding.binding) else {
                    return Ok(implementations);
                };
                Self::extend_impl_refs(
                    &mut implementations,
                    implementation_query.impls_for_ty(binding_ty)?,
                );
            }
            DeclarationRef::Module(_)
            | DeclarationRef::Field(_)
            | DeclarationRef::EnumVariant(_) => {}
        }

        Ok(implementations)
    }

    fn implementations_for_ty(
        &self,
        use_site: TargetRef,
        ty: &Ty,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let implementation_query = ImplementationQuery::new(
            ItemPathQuery::new(self.0.view_db(), self.0.view_db()),
            TargetItemQuery::new(self.0.view_db(), self.0.view_db(), use_site),
        );
        let mut implementations = Vec::new();
        Self::extend_impl_refs(&mut implementations, implementation_query.impls_for_ty(ty)?);
        Ok(implementations)
    }

    fn function_ref_for_declaration(
        &self,
        declaration: DeclarationRef,
    ) -> anyhow::Result<Option<FunctionRef>> {
        match declaration {
            DeclarationRef::LocalDef(local_def) => Ok(ItemStoreQuery::new(self.0.view_db())
                .semantic_item_for_local_def(local_def)?
                .and_then(|item| match item {
                    SemanticItemRef::Function(function) => Some(function),
                    _ => None,
                })),
            DeclarationRef::Item(SemanticItemRef::Function(function)) => Ok(Some(function)),
            DeclarationRef::Module(_)
            | DeclarationRef::Item(_)
            | DeclarationRef::Field(_)
            | DeclarationRef::EnumVariant(_)
            | DeclarationRef::BodyBinding(_) => Ok(None),
        }
    }

    fn extend_impl_refs(implementations: &mut Vec<DeclarationRef>, impls: Vec<ImplRef>) {
        for impl_ref in impls {
            Self::push_unique_declaration(implementations, DeclarationRef::from(impl_ref));
        }
    }

    fn extend_function_refs(
        implementations: &mut Vec<DeclarationRef>,
        functions: Vec<FunctionRef>,
    ) {
        for function in functions {
            Self::push_unique_declaration(implementations, DeclarationRef::from(function));
        }
    }

    fn extend_unique_declarations(
        declarations: &mut Vec<DeclarationRef>,
        new_declarations: Vec<DeclarationRef>,
    ) {
        for declaration in new_declarations {
            Self::push_unique_declaration(declarations, declaration);
        }
    }

    fn push_unique_declaration(
        declarations: &mut Vec<DeclarationRef>,
        declaration: DeclarationRef,
    ) {
        if !declarations.contains(&declaration) {
            declarations.push(declaration);
        }
    }
}
