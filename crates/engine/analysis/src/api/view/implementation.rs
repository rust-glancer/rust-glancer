//! Composite implementation lookup over semantic and body-local declarations.
//!
//! Goto-implementation asks for source declarations that implement a selected type, trait, or
//! trait method. This view owns the storage-specific lookup rules so query code can stay focused on
//! cursor policy and editor projection.

use rg_body_ir::{
    BodyAutoderef, BodyAutoderefMode, BodyDeclarationRef, BodyImplId, BodyImplRef, BodyResolution,
    BodyTy, BodyTyExt, ExprKind, ResolvedDeclarationRef,
};
use rg_def_map::DefId;
use rg_semantic_ir::{
    AssocItemId, FunctionRef, ImplRef, ItemOwner, SemanticDeclarationRef, SemanticItemRef,
    TraitRef, TypeDefRef,
};

use crate::{
    api::{Analysis, view::declaration::DeclarationRef},
    model::SymbolAt,
};

pub(crate) struct ImplementationView<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> ImplementationView<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self { analysis }
    }

    pub(crate) fn implementations_for_method_call(
        &self,
        symbol: &SymbolAt,
    ) -> anyhow::Result<Option<Vec<DeclarationRef>>> {
        let SymbolAt::Expr { body, expr } = *symbol else {
            return Ok(None);
        };
        let Some(body_data) = self.analysis.body_ir.body_data(body)? else {
            return Ok(None);
        };
        let Some(expr_data) = body_data.expr(expr) else {
            return Ok(None);
        };
        let ExprKind::MethodCall {
            receiver: Some(receiver),
            ..
        } = &expr_data.kind
        else {
            return Ok(None);
        };
        let BodyResolution::Method(functions) = &expr_data.resolution else {
            return Ok(None);
        };

        let receiver_ty = body_data.expr(*receiver).map(|data| &data.ty);
        let mut implementations = Vec::new();
        for declaration in functions {
            self.extend_resolved_function_implementations(
                &mut implementations,
                *declaration,
                receiver_ty,
            )?;
        }
        Ok(Some(implementations))
    }

    pub(crate) fn implementations_for_declaration(
        &self,
        declaration: DeclarationRef,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let mut implementations = Vec::new();

        match declaration {
            DeclarationRef::Semantic(SemanticDeclarationRef::Item(item)) => match item {
                SemanticItemRef::TypeDef(ty) => {
                    self.extend_type_def_implementations(&mut implementations, ty)?;
                }
                SemanticItemRef::Trait(trait_ref) => {
                    self.extend_trait_implementations(&mut implementations, trait_ref)?;
                }
                SemanticItemRef::Function(function) => {
                    self.extend_semantic_function_implementations(
                        &mut implementations,
                        function,
                        None,
                    )?;
                }
                SemanticItemRef::Impl(_)
                | SemanticItemRef::TypeAlias(_)
                | SemanticItemRef::Const(_)
                | SemanticItemRef::Static(_) => {}
            },
            DeclarationRef::Body(declaration) => match declaration {
                BodyDeclarationRef::Binding(_) => {
                    let Some(view) = self.analysis.body_ir.body_declaration_view(declaration)?
                    else {
                        return Ok(implementations);
                    };
                    let Some(binding_data) = view.binding_data() else {
                        return Ok(implementations);
                    };
                    self.extend_ty_implementations(&mut implementations, &binding_data.ty)?;
                }
                BodyDeclarationRef::Item(item) => {
                    self.extend_local_type_implementations(&mut implementations, item)?;
                }
                BodyDeclarationRef::Function(function) => {
                    self.extend_resolved_function_implementations(
                        &mut implementations,
                        function.into(),
                        None,
                    )?;
                }
                BodyDeclarationRef::ValueItem(_)
                | BodyDeclarationRef::Impl(_)
                | BodyDeclarationRef::Field(_)
                | BodyDeclarationRef::EnumVariant(_) => {}
            },
            DeclarationRef::Semantic(
                SemanticDeclarationRef::Field(_) | SemanticDeclarationRef::EnumVariant(_),
            )
            | DeclarationRef::Module(_)
            | DeclarationRef::LocalDef(_) => {}
        }

        Ok(implementations)
    }

    pub(crate) fn implementations_for_ty(
        &self,
        ty: &BodyTy,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let mut implementations = Vec::new();
        self.extend_ty_implementations(&mut implementations, ty)?;
        Ok(implementations)
    }

    fn extend_ty_implementations(
        &self,
        implementations: &mut Vec<DeclarationRef>,
        ty: &BodyTy,
    ) -> anyhow::Result<()> {
        for candidate in BodyAutoderef::peel_references(ty) {
            for local_ty in candidate.ty().as_local_nominals() {
                self.extend_local_type_implementations(implementations, local_ty.item)?;
            }
        }
        for candidate in BodyAutoderef::peel_references(ty) {
            for ty in candidate.ty().as_nominals() {
                self.extend_type_def_implementations(implementations, ty.def)?;
            }
        }
        Ok(())
    }

    fn extend_type_def_implementations(
        &self,
        implementations: &mut Vec<DeclarationRef>,
        ty: TypeDefRef,
    ) -> anyhow::Result<()> {
        for impl_ref in self.analysis.semantic_ir.impls_for_type(ty)? {
            Self::push_unique(implementations, impl_ref.into());
        }
        Ok(())
    }

    fn extend_local_type_implementations(
        &self,
        implementations: &mut Vec<DeclarationRef>,
        item: rg_body_ir::BodyItemRef,
    ) -> anyhow::Result<()> {
        let Some(body) = self.analysis.body_ir.body_data(item.body)? else {
            return Ok(());
        };

        for (impl_idx, data) in body.local_impls().iter().enumerate() {
            if data.self_item != Some(item) {
                continue;
            }
            Self::push_unique(
                implementations,
                BodyImplRef {
                    body: item.body,
                    impl_id: BodyImplId(impl_idx),
                }
                .into(),
            );
        }
        Ok(())
    }

    fn extend_trait_implementations(
        &self,
        implementations: &mut Vec<DeclarationRef>,
        trait_ref: TraitRef,
    ) -> anyhow::Result<()> {
        for impl_ref in self.analysis.semantic_ir.impls_for_trait(trait_ref)? {
            Self::push_unique(implementations, impl_ref.into());
        }
        Ok(())
    }

    fn extend_resolved_function_implementations(
        &self,
        implementations: &mut Vec<DeclarationRef>,
        declaration: ResolvedDeclarationRef,
        receiver_ty: Option<&BodyTy>,
    ) -> anyhow::Result<()> {
        match declaration {
            ResolvedDeclarationRef::Def(DefId::Local(local_def)) => {
                let Some(SemanticItemRef::Function(function)) = self
                    .analysis
                    .semantic_ir
                    .semantic_item_for_local_def(local_def)?
                else {
                    return Ok(());
                };
                self.extend_semantic_function_implementations(
                    implementations,
                    function,
                    receiver_ty,
                )
            }
            ResolvedDeclarationRef::Def(DefId::Module(_)) => Ok(()),
            ResolvedDeclarationRef::Semantic(SemanticDeclarationRef::Item(
                SemanticItemRef::Function(function),
            )) => self.extend_semantic_function_implementations(
                implementations,
                function,
                receiver_ty,
            ),
            ResolvedDeclarationRef::Body(BodyDeclarationRef::Function(function)) => {
                Self::push_unique(implementations, function.into());
                Ok(())
            }
            ResolvedDeclarationRef::Semantic(
                SemanticDeclarationRef::Item(
                    SemanticItemRef::TypeDef(_)
                    | SemanticItemRef::Trait(_)
                    | SemanticItemRef::Impl(_)
                    | SemanticItemRef::TypeAlias(_)
                    | SemanticItemRef::Const(_)
                    | SemanticItemRef::Static(_),
                )
                | SemanticDeclarationRef::Field(_)
                | SemanticDeclarationRef::EnumVariant(_),
            )
            | ResolvedDeclarationRef::Body(
                BodyDeclarationRef::Binding(_)
                | BodyDeclarationRef::Item(_)
                | BodyDeclarationRef::ValueItem(_)
                | BodyDeclarationRef::Impl(_)
                | BodyDeclarationRef::Field(_)
                | BodyDeclarationRef::EnumVariant(_),
            ) => Ok(()),
        }
    }

    fn extend_semantic_function_implementations(
        &self,
        implementations: &mut Vec<DeclarationRef>,
        function: FunctionRef,
        receiver_ty: Option<&BodyTy>,
    ) -> anyhow::Result<()> {
        let Some(data) = self.analysis.semantic_ir.function_data(function)? else {
            return Ok(());
        };

        match data.owner {
            ItemOwner::Trait(trait_id) => {
                let trait_ref = TraitRef {
                    target: function.target,
                    id: trait_id,
                };
                match receiver_ty {
                    Some(receiver_ty) => self.extend_trait_method_implementations_for_receiver(
                        implementations,
                        trait_ref,
                        data.name.as_str(),
                        receiver_ty,
                    ),
                    None => self.extend_trait_method_implementations(
                        implementations,
                        trait_ref,
                        data.name.as_str(),
                    ),
                }
            }
            ItemOwner::Impl(_) => {
                Self::push_unique(implementations, function.into());
                Ok(())
            }
            ItemOwner::Module(_) => Ok(()),
        }
    }

    fn extend_trait_method_implementations_for_receiver(
        &self,
        implementations: &mut Vec<DeclarationRef>,
        trait_ref: TraitRef,
        method_name: &str,
        receiver_ty: &BodyTy,
    ) -> anyhow::Result<()> {
        let autoderef = BodyAutoderef::new(&self.analysis.def_map, &self.analysis.semantic_ir);
        for candidate in autoderef.candidates(BodyAutoderefMode::MethodReceiver, receiver_ty) {
            let candidate = candidate?;
            for ty in candidate.ty().as_nominals() {
                for trait_impl in self.analysis.semantic_ir.trait_impls_for_type(ty.def)? {
                    if trait_impl.trait_ref != trait_ref {
                        continue;
                    }
                    // The nominal type match can still include generic impls for other concrete
                    // args. Reuse method lookup's applicability check so goto-implementation
                    // follows the receiver the user actually called the method on.
                    if !self
                        .analysis
                        .body_ir
                        .semantic_trait_impl_applies_to_receiver(
                            &self.analysis.def_map,
                            &self.analysis.semantic_ir,
                            trait_impl,
                            ty,
                        )?
                    {
                        continue;
                    }
                    self.extend_matching_impl_methods(
                        implementations,
                        trait_impl.impl_ref,
                        method_name,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn extend_trait_method_implementations(
        &self,
        implementations: &mut Vec<DeclarationRef>,
        trait_ref: TraitRef,
        method_name: &str,
    ) -> anyhow::Result<()> {
        for impl_ref in self.analysis.semantic_ir.impls_for_trait(trait_ref)? {
            self.extend_matching_impl_methods(implementations, impl_ref, method_name)?;
        }
        Ok(())
    }

    fn extend_matching_impl_methods(
        &self,
        implementations: &mut Vec<DeclarationRef>,
        impl_ref: ImplRef,
        method_name: &str,
    ) -> anyhow::Result<()> {
        let Some(data) = self.analysis.semantic_ir.impl_data(impl_ref)? else {
            return Ok(());
        };

        for item in &data.items {
            let AssocItemId::Function(id) = item else {
                continue;
            };
            let function = FunctionRef {
                target: impl_ref.target,
                id: *id,
            };
            let Some(function_data) = self.analysis.semantic_ir.function_data(function)? else {
                continue;
            };
            if function_data.name.as_str() != method_name {
                continue;
            }
            Self::push_unique(implementations, function.into());
        }
        Ok(())
    }

    fn push_unique(implementations: &mut Vec<DeclarationRef>, declaration: DeclarationRef) {
        if !implementations.contains(&declaration) {
            implementations.push(declaration);
        }
    }
}
