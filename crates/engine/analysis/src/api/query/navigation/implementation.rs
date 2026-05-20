//! Goto-implementation query flow.

use rg_body_ir::{BodyImplId, BodyResolution, BodyTy, ExprKind, ResolvedFunctionRef};
use rg_def_map::TargetRef;
use rg_parse::FileId;
use rg_semantic_ir::{AssocItemId, FunctionRef, ImplRef, ItemOwner, TraitRef, TypeDefRef};

use super::target::NavigationTargetResolver;
use crate::{
    api::{
        Analysis,
        query::type_at::TypeResolver,
        resolve::entity::{EntityResolver, ResolvedEntity},
    },
    model::{NavigationTarget, SymbolAt},
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

        let mut targets = Vec::new();
        // 1) Method-call sites: prefer concrete callable implementations.
        if self.push_method_call_targets(&symbol, &mut targets)? {
            return Ok(targets);
        }

        // 2) Symbol/entity expansion: trait/type/function/local bindings.
        for entity in EntityResolver::new(self.0).entities_for_symbol(symbol)? {
            self.push_entity_targets(entity, &mut targets)?;
        }

        // 3) If symbol expansion found nothing, fall back to inferred type.
        if targets.is_empty()
            && let Some(ty) = TypeResolver::new(self.0).type_at(target, file_id, offset)?
        {
            self.push_ty_targets(&ty, &mut targets)?;
        }

        Ok(targets)
    }

    fn push_method_call_targets(
        &self,
        symbol: &SymbolAt,
        targets: &mut Vec<NavigationTarget>,
    ) -> anyhow::Result<bool> {
        let SymbolAt::Expr { body, expr } = *symbol else {
            return Ok(false);
        };
        let Some(body_data) = self.0.body_ir.body_data(body)? else {
            return Ok(false);
        };
        let Some(expr_data) = body_data.expr(expr) else {
            return Ok(false);
        };
        let ExprKind::MethodCall {
            receiver: Some(receiver),
            ..
        } = &expr_data.kind
        else {
            return Ok(false);
        };
        let BodyResolution::Method(functions) = &expr_data.resolution else {
            return Ok(false);
        };
        let receiver_ty = body_data.expr(*receiver).map(|data| &data.ty);

        for function in functions {
            self.push_resolved_function_targets(*function, receiver_ty, targets)?;
        }

        Ok(true)
    }

    fn push_entity_targets(
        &self,
        entity: ResolvedEntity,
        targets: &mut Vec<NavigationTarget>,
    ) -> anyhow::Result<()> {
        match entity {
            ResolvedEntity::TypeDef(ty) => self.push_type_def_targets(ty, targets),
            ResolvedEntity::Trait(trait_ref) => self.push_trait_impl_targets(trait_ref, targets),
            ResolvedEntity::Function(function) => {
                self.push_resolved_function_targets(function, None, targets)
            }
            ResolvedEntity::LocalBinding { body, binding } => {
                let Some(body_data) = self.0.body_ir.body_data(body)? else {
                    return Ok(());
                };
                let Some(binding_data) = body_data.binding(binding) else {
                    return Ok(());
                };
                self.push_ty_targets(&binding_data.ty, targets)
            }
            ResolvedEntity::LocalItem(item) => self.push_local_type_targets(item, targets),
            ResolvedEntity::Module { .. }
            | ResolvedEntity::Field(_)
            | ResolvedEntity::EnumVariant(_)
            | ResolvedEntity::TypeAlias(_)
            | ResolvedEntity::Const(_)
            | ResolvedEntity::Static(_)
            | ResolvedEntity::LocalValueItem(_)
            | ResolvedEntity::LocalDef(_) => Ok(()),
        }
    }

    fn push_ty_targets(
        &self,
        ty: &BodyTy,
        targets: &mut Vec<NavigationTarget>,
    ) -> anyhow::Result<()> {
        for local_ty in ty.local_nominals() {
            self.push_local_type_targets(local_ty.item, targets)?;
        }
        for ty in ty.nominal_tys() {
            self.push_type_def_targets(ty.def, targets)?;
        }
        Ok(())
    }

    fn push_type_def_targets(
        &self,
        ty: TypeDefRef,
        targets: &mut Vec<NavigationTarget>,
    ) -> anyhow::Result<()> {
        for impl_ref in self.0.semantic_ir.impls_for_type(ty)? {
            self.push_impl_target(impl_ref, targets)?;
        }
        Ok(())
    }

    fn push_local_type_targets(
        &self,
        item: rg_body_ir::BodyItemRef,
        targets: &mut Vec<NavigationTarget>,
    ) -> anyhow::Result<()> {
        let Some(body) = self.0.body_ir.body_data(item.body)? else {
            return Ok(());
        };

        for (impl_idx, data) in body.local_impls().iter().enumerate() {
            if data.self_item != Some(item) {
                continue;
            }
            let Some(target) = NavigationTargetResolver::new(self.0)
                .navigation_target_for_body_impl(item.body, BodyImplId(impl_idx))?
            else {
                continue;
            };
            Self::push_unique_target(targets, target);
        }
        Ok(())
    }

    fn push_trait_impl_targets(
        &self,
        trait_ref: TraitRef,
        targets: &mut Vec<NavigationTarget>,
    ) -> anyhow::Result<()> {
        for impl_ref in self.0.semantic_ir.impls_for_trait(trait_ref)? {
            self.push_impl_target(impl_ref, targets)?;
        }
        Ok(())
    }

    fn push_resolved_function_targets(
        &self,
        function: ResolvedFunctionRef,
        receiver_ty: Option<&BodyTy>,
        targets: &mut Vec<NavigationTarget>,
    ) -> anyhow::Result<()> {
        match function {
            ResolvedFunctionRef::Semantic(function) => {
                self.push_semantic_function_targets(function, receiver_ty, targets)
            }
            ResolvedFunctionRef::BodyLocal(function) => {
                let Some(target) = NavigationTargetResolver::new(self.0)
                    .navigation_target_for_resolved_function(ResolvedFunctionRef::BodyLocal(
                        function,
                    ))?
                else {
                    return Ok(());
                };
                Self::push_unique_target(targets, target);
                Ok(())
            }
        }
    }

    fn push_semantic_function_targets(
        &self,
        function: FunctionRef,
        receiver_ty: Option<&BodyTy>,
        targets: &mut Vec<NavigationTarget>,
    ) -> anyhow::Result<()> {
        let Some(data) = self.0.semantic_ir.function_data(function)? else {
            return Ok(());
        };

        match data.owner {
            ItemOwner::Trait(trait_id) => {
                let trait_ref = TraitRef {
                    target: function.target,
                    id: trait_id,
                };
                match receiver_ty {
                    Some(receiver_ty) => self.push_trait_method_targets_for_receiver(
                        trait_ref,
                        data.name.as_str(),
                        receiver_ty,
                        targets,
                    ),
                    None => self.push_trait_method_targets(trait_ref, data.name.as_str(), targets),
                }
            }
            ItemOwner::Impl(_) => {
                let Some(target) = NavigationTargetResolver::new(self.0)
                    .navigation_target_for_function(function)?
                else {
                    return Ok(());
                };
                Self::push_unique_target(targets, target);
                Ok(())
            }
            ItemOwner::Module(_) => Ok(()),
        }
    }

    fn push_trait_method_targets_for_receiver(
        &self,
        trait_ref: TraitRef,
        method_name: &str,
        receiver_ty: &BodyTy,
        targets: &mut Vec<NavigationTarget>,
    ) -> anyhow::Result<()> {
        for ty in receiver_ty.nominal_tys() {
            for trait_impl in self.0.semantic_ir.trait_impls_for_type(ty.def)? {
                if trait_impl.trait_ref != trait_ref {
                    continue;
                }
                // The nominal type match can still include generic impls for other concrete args.
                // Reuse method lookup's applicability check so `Wrapper<Account>` does not jump to
                // implementations that only apply to `Wrapper<User>`.
                if !self.0.body_ir.semantic_trait_impl_applies_to_receiver(
                    &self.0.def_map,
                    &self.0.semantic_ir,
                    trait_impl,
                    ty,
                )? {
                    continue;
                }
                self.push_matching_impl_methods(trait_impl.impl_ref, method_name, targets)?;
            }
        }
        Ok(())
    }

    fn push_trait_method_targets(
        &self,
        trait_ref: TraitRef,
        method_name: &str,
        targets: &mut Vec<NavigationTarget>,
    ) -> anyhow::Result<()> {
        for impl_ref in self.0.semantic_ir.impls_for_trait(trait_ref)? {
            self.push_matching_impl_methods(impl_ref, method_name, targets)?;
        }
        Ok(())
    }

    fn push_matching_impl_methods(
        &self,
        impl_ref: ImplRef,
        method_name: &str,
        targets: &mut Vec<NavigationTarget>,
    ) -> anyhow::Result<()> {
        let Some(data) = self.0.semantic_ir.impl_data(impl_ref)? else {
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
            let Some(function_data) = self.0.semantic_ir.function_data(function)? else {
                continue;
            };
            if function_data.name.as_str() != method_name {
                continue;
            }
            let Some(target) =
                NavigationTargetResolver::new(self.0).navigation_target_for_function(function)?
            else {
                continue;
            };
            Self::push_unique_target(targets, target);
        }
        Ok(())
    }

    fn push_impl_target(
        &self,
        impl_ref: ImplRef,
        targets: &mut Vec<NavigationTarget>,
    ) -> anyhow::Result<()> {
        let Some(target) =
            NavigationTargetResolver::new(self.0).navigation_target_for_impl(impl_ref)?
        else {
            return Ok(());
        };
        Self::push_unique_target(targets, target);
        Ok(())
    }

    fn push_unique_target(targets: &mut Vec<NavigationTarget>, target: NavigationTarget) {
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
}
