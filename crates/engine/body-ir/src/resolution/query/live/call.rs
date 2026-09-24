//! Call lookup retains the trial context that established a receiver's applicability.
//!
//! For `value.method()`, try receiver adjustments in order. `&&Widget` can expose candidates at
//! `&&Widget`, `&Widget`, or `Widget`; the first depth with candidates wins. Within that depth,
//! inherent methods take precedence over methods from visible traits. The chosen target retains
//! its receiver bindings so its signature uses the same variables as the body.
use rg_def_map::DefMapSource;
use rg_ir_model::{
    AssocItemId, ExprId, FunctionRef, ScopeId, SemanticItemRef, identity::DeclarationRef,
};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::UniqueVec;
use rg_ty::solver::{
    DefId, InferenceSubstitution, InferenceTable, Outcome, TraitApplication, Ty, TyShape,
};

use super::LiveBodyQuery;
use crate::{
    BodyAssociatedPathPrefix,
    body::{ExprKind, facts::BodyResolution},
    resolution::cache::BodyTraitSurface,
};

/// A function candidate together with the receiver evidence found while considering it.
/// Its table is a separate trial: competing candidates must not constrain one another. If call
/// inference chooses this target, it adopts the table and reuses the receiver's substitutions.
pub(crate) struct LiveCallTarget<'s> {
    pub function: FunctionRef,
    pub explicit_args: Vec<rg_item_tree::GenericArg>,
    pub scope: ScopeId,
    pub subst: InferenceSubstitution<'s>,
    pub receiver: Option<Ty<'s>>,
    pub first_written: usize,
    pub table: InferenceTable<'s>,
    // Lookup can retain a declaration for navigation even when its trial cannot supply types.
    pub can_infer: bool,
}

impl<'query, D, I> LiveBodyQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    /// Collect call targets, stopping at the first matching receiver adjustment for a method.
    /// Several candidates remain useful for navigation; call inference requires a unique usable
    /// target before it can connect a signature to the body's argument and result variables.
    pub(crate) fn call_targets<'s>(
        &self,
        call: ExprId,
        resolution: Option<&BodyResolution>,
        receiver: Option<Ty<'s>>,
        table: &InferenceTable<'s>,
    ) -> Result<Vec<LiveCallTarget<'s>>, PackageStoreError> {
        let data = self.context.body().expr_unchecked(call);
        match &data.kind {
            ExprKind::MethodCall {
                method_name,
                generic_args,
                ..
            } => {
                let Some(receiver) = receiver else {
                    return Ok(Vec::new());
                };
                for receiver in self.receivers(receiver, table, true)? {
                    let candidates = self.named_targets(
                        data.scope,
                        receiver,
                        method_name,
                        true,
                        generic_args,
                        None,
                        table,
                    )?;
                    if !candidates.is_empty() {
                        return Ok(candidates);
                    }
                }
                Ok(Vec::new())
            }
            ExprKind::Call {
                callee: Some(callee),
                ..
            } => {
                let callee = self.context.body().expr_unchecked(*callee);
                let explicit = match &callee.kind {
                    ExprKind::Path { path } => path.last_segment_angle_args().unwrap_or(&[]),
                    _ => &[],
                };
                if let ExprKind::Path { path } = &callee.kind
                    && let Some((prefix, name)) = path.split_associated_item_prefix_name()
                {
                    let (receiver, qualification) = match prefix {
                        BodyAssociatedPathPrefix::Type(ty) => {
                            (self.type_ref(callee.scope, &ty, table)?, None)
                        }
                        BodyAssociatedPathPrefix::QualifiedTrait { self_ty, trait_ref } => {
                            let owned = self.context.type_refs(callee.scope).resolve(&self_ty)?;
                            let tr = self
                                .context
                                .type_refs(callee.scope)
                                .resolve_trait_ref(&trait_ref, owned.clone())?;
                            let owner = self.context.body().owner().generic_def();
                            let qualification = tr.map(|tr| TraitApplication {
                                def: tr.application.def,
                                args: table
                                    .interner()
                                    .lower_args(&tr.application.args, table.params(owner.into())),
                            });
                            (table.lower(&owned, owner.into()), qualification)
                        }
                    };
                    if !receiver.is_unknown() || qualification.is_some() {
                        let receiver = table.instantiate_nested_unknowns(receiver);
                        let targets = self.named_targets(
                            callee.scope,
                            receiver,
                            name,
                            false,
                            explicit,
                            qualification,
                            table,
                        )?;
                        if !targets.is_empty() {
                            return Ok(targets);
                        }
                    }
                }
                let mut targets = Vec::new();
                if let Some(BodyResolution::Declarations(declarations)) = resolution {
                    for declaration in declarations {
                        if let Some(function) = self.declaration_function(*declaration)? {
                            targets.push(LiveCallTarget {
                                function,
                                explicit_args: explicit.to_vec(),
                                scope: callee.scope,
                                subst: InferenceSubstitution::new(),
                                receiver: None,
                                first_written: 0,
                                table: table.probe(),
                                can_infer: true,
                            });
                        }
                    }
                }
                Ok(targets)
            }
            _ => Ok(Vec::new()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn named_targets<'s>(
        &self,
        scope: ScopeId,
        receiver: Ty<'s>,
        name: &str,
        method: bool,
        explicit: &[rg_item_tree::GenericArg],
        qualification: Option<TraitApplication<'s>>,
        table: &InferenceTable<'s>,
    ) -> Result<Vec<LiveCallTarget<'s>>, PackageStoreError> {
        let mut targets = Vec::new();
        let body_items = self.context.body_local_items();
        let lookup = self.context.item_lookup_query();
        // Unqualified calls try inherent declarations first. A written `<T as Trait>::method`
        // already chooses the trait, so it bypasses that search.
        if qualification.is_none() {
            let mut impls = UniqueVec::new();
            let mut shadowed = None;
            if let Some(adt) = receiver.as_adt() {
                impls.extend(body_items.inherent_impls_for_type(adt.def)?.iter().copied());
                shadowed = body_items.inherent_item_names_for_type(adt.def)?;
                if let Ok(saved) = lookup.inherent_impls_for_type(adt.def) {
                    impls.extend(saved);
                }
            } else if !matches!(
                receiver.shape(),
                TyShape::InferVar { .. } | TyShape::Unknown | TyShape::Param(_) | TyShape::Alias(_)
            ) && let Ok(saved) = lookup.structural_inherent_impls()
            {
                impls.extend(saved);
            }
            for id in impls {
                if id.origin.as_crate_ref().is_some()
                    && shadowed
                        .as_ref()
                        .is_some_and(|names| names.contains_function(name))
                {
                    continue;
                }
                let Some(data) = self.context.item_query().impl_data(id)? else {
                    continue;
                };
                if data.trait_ref.is_some() {
                    continue;
                }

                // Look for the requested declaration before running any semantic probe.
                for item in &data.items {
                    let AssocItemId::Function(function_id) = item else {
                        continue;
                    };
                    let function = FunctionRef {
                        origin: id.origin,
                        id: *function_id,
                    };
                    let Some(function_data) = self.context.item_query().function_data(function)?
                    else {
                        continue;
                    };
                    if function_data.name != name || method && !function_data.has_self_receiver() {
                        continue;
                    }
                    let Some(selection) = table.select_impl(id, receiver, None) else {
                        continue;
                    };
                    targets.push(LiveCallTarget {
                        function,
                        explicit_args: explicit.to_vec(),
                        scope,
                        subst: selection.subst,
                        receiver: Some(receiver),
                        first_written: usize::from(method),
                        table: selection.table,
                        // `Wrapper::new(value)` may learn the impl's T from the argument. A
                        // unique inherent header supplies its signature now; the adopted table
                        // keeps its predicates pending until that argument provides evidence.
                        can_infer: matches!(
                            selection.outcome,
                            Outcome::Proven | Outcome::Ambiguous
                        ),
                    });
                }
            }
            if !targets.is_empty() {
                return Ok(targets);
            }
        }
        // Search only traits that can supply this name, then ask whether the receiver implements
        // each one. Name lookup narrows the declarations; the trial carries the type evidence.
        let traits = match qualification {
            Some(tr) => vec![tr.def],
            None => self
                .context
                .impls()
                .trait_refs_for_surface(scope, BodyTraitSurface::FunctionNamed(name))?
                .iter()
                .copied()
                .collect(),
        };
        for trait_ref in traits {
            let Some(data) = self.context.item_query().trait_data(trait_ref)? else {
                continue;
            };
            for item in &data.items {
                let AssocItemId::Function(function_id) = item else {
                    continue;
                };
                let function = FunctionRef {
                    origin: trait_ref.origin,
                    id: *function_id,
                };
                let Some(data) = self.context.item_query().function_data(function)? else {
                    continue;
                };
                if data.name != name || method && !data.has_self_receiver() {
                    continue;
                }
                let trial = table.probe();
                let owner = DefId::Trait(trait_ref);
                let mut subst = qualification.map_or_else(
                    || trial.fresh_substitution(owner),
                    |tr| {
                        InferenceSubstitution::from_args(
                            trial.params(owner).iter().copied(),
                            tr.args,
                        )
                    },
                );
                if let Some(self_param) = trial.params(owner).first() {
                    subst.insert(*self_param, receiver.into());
                }
                let args = subst.args_for(trial.interner(), trial.params(owner).iter().copied());
                let outcome = trial.prove([TraitApplication {
                    def: trait_ref,
                    args,
                }
                .clause(trial.interner())]);
                if outcome == Outcome::NoSolution {
                    continue;
                }
                targets.push(LiveCallTarget {
                    function,
                    explicit_args: explicit.to_vec(),
                    scope,
                    subst,
                    receiver: Some(receiver),
                    first_written: usize::from(method),
                    table: trial,
                    // A unique trait declaration supplies a signature even while its proof is
                    // pending. `handler.convert(value)` can learn the trait's T from value;
                    // `Widget::default()` already knows Self from its written receiver. Keep
                    // the obligation in the adopted table instead of requiring proof before
                    // arguments can provide evidence. Competing declarations remain ambiguous.
                    can_infer: true,
                });
            }
        }
        Ok(targets)
    }

    /// Keep only declarations that name functions.
    fn declaration_function(
        &self,
        declaration: DeclarationRef,
    ) -> Result<Option<FunctionRef>, PackageStoreError> {
        match declaration {
            DeclarationRef::LocalDef(local_def) => Ok(
                match self
                    .context
                    .item_query()
                    .semantic_item_for_local_def(local_def)?
                {
                    Some(SemanticItemRef::Function(function)) => Some(function),
                    Some(_) | None => None,
                },
            ),
            DeclarationRef::Item(SemanticItemRef::Function(function_ref)) => Ok(Some(function_ref)),
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
            | DeclarationRef::BodyBinding(_) => Ok(None),
        }
    }
}
